//! The Tenant Secret role of the Credential Broker (ADR 0095, ADR 0096).
//!
//! It verifies a Tenant Secret Capability Token, reads the one sealed Secret
//! Store version the token names, checks the token's destination against the
//! hosts sealed with that version, attaches the value and streams the call.
//! It never accepts a secret, destination or Tenant from the request, and it
//! shares no configuration or identity with the Key Material role.
use crate::Bucket;
use aidash_capability::{
	KeyMaterialError, PublicKeys,
	tenant_secret::{Attach, SealedSecretSource, SecretClaims, SecretSubject, sealed},
};
use aidash_domain::keyless::{Destination, PermittedOrigins};
use axum::{
	Router,
	body::{Body, Bytes},
	extract::{Request, State},
	http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
	response::Response,
};
use futures_util::StreamExt;
use percent_encoding::percent_decode_str;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use std::{
	collections::{BTreeMap, HashMap},
	sync::{
		Arc, Mutex,
		atomic::{AtomicU64, Ordering},
	},
	time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

const SECRET_CALL: &str = "x-aidash-secret-call";
const BROKER_ERROR: &str = "x-aidash-broker-error";
const CORRELATION: &str = "x-aidash-correlation";
const FORWARD: &str = "/v1/forward";
const CACHE_TTL: Duration = Duration::from_secs(60);
const PATH_DIGEST_CHARS: usize = 16;

const BROKER_REJECTED: &str = "broker_rejected";
const DESTINATION_REFUSED: &str = "secret_destination_refused";
const SECRET_UNAVAILABLE: &str = "secret_unavailable";
const RATE_LIMITED: &str = "rate_limited";
const BROKER_UNAVAILABLE: &str = "broker_unavailable";
const UPSTREAM_UNREACHABLE: &str = "upstream_unreachable";

/// The Tenant Secret role's configuration. `issuers` maps each accepted `kid`
/// to the one issuer that key may speak for; the Key Material role has no
/// equivalent because it has a single issuer.
#[derive(Clone)]
pub struct TenantSecretConfig {
	pub byok_project_number: String,
	pub secret_prefix: String,
	pub audience: String,
	pub issuers: BTreeMap<String, String>,
	pub requests_per_second: f64,
	pub burst: u32,
	pub timeout: Duration,
}
impl TenantSecretConfig {
	fn valid(&self) -> bool {
		!self.byok_project_number.is_empty()
			&& self.byok_project_number.bytes().all(|b| b.is_ascii_digit())
			&& self.secret_prefix.starts_with("aidash-")
			&& self.secret_prefix.ends_with("-tsec-")
			&& self
				.secret_prefix
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
			&& !self.audience.is_empty()
			&& self.audience.len() <= 64
			&& self
				.audience
				.bytes()
				.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
			&& !self.issuers.is_empty()
			&& self
				.issuers
				.iter()
				.all(|(kid, issuer)| !kid.is_empty() && !issuer.is_empty() && issuer.len() <= 256)
			&& self.requests_per_second.is_finite()
			&& self.requests_per_second > 0.0
			&& self.burst > 0
			&& !self.timeout.is_zero()
			&& self.timeout <= Duration::from_secs(3600)
	}
}

/// One structured `secret_call` line. Every field is required by the contract;
/// the struct never carries a value, a header value or a body.
#[derive(Debug, serde::Serialize)]
pub struct SecretCallAudit {
	pub tenant: String,
	pub secret: uuid::Uuid,
	pub version: String,
	pub host: String,
	pub path_sha256: String,
	pub method: String,
	pub status: u16,
	pub bytes_in: u64,
	pub bytes_out: u64,
	pub jti: uuid::Uuid,
	pub subject: String,
	pub duration_ms: u128,
}
pub trait SecretCallSink: Send + Sync {
	fn record(&self, audit: &SecretCallAudit);
}

struct CachedSealed {
	hosts: Vec<String>,
	value: SecretString,
	since: Instant,
}
struct AuditGuard {
	started: Instant,
	sink: Arc<dyn SecretCallSink>,
	entry: SecretCallAudit,
	bytes_in: Arc<AtomicU64>,
	bytes_out: Arc<AtomicU64>,
}
impl Drop for AuditGuard {
	fn drop(&mut self) {
		self.entry.bytes_in = self.bytes_in.load(Ordering::Relaxed);
		self.entry.bytes_out = self.bytes_out.load(Ordering::Relaxed);
		self.entry.duration_ms = self.started.elapsed().as_millis();
		self.sink.record(&self.entry);
	}
}
impl AuditGuard {
	fn new(sink: Arc<dyn SecretCallSink>, started: Instant, entry: SecretCallAudit) -> Self {
		Self {
			started,
			sink,
			entry,
			bytes_in: Arc::default(),
			bytes_out: Arc::default(),
		}
	}
	fn status(&mut self, status: StatusCode) {
		self.entry.status = status.as_u16();
	}
	fn error(
		&mut self,
		status: StatusCode,
		code: &'static str,
		jti: Option<uuid::Uuid>,
	) -> Response {
		self.status(status);
		broker_error(status, code, jti)
	}
}

pub struct TenantSecretBroker {
	config: TenantSecretConfig,
	keys: PublicKeys,
	source: Arc<dyn SealedSecretSource>,
	audit: Arc<dyn SecretCallSink>,
	client: reqwest::Client,
	cache: Arc<Mutex<HashMap<(String, String), CachedSealed>>>,
	buckets: Mutex<HashMap<(String, uuid::Uuid), Bucket>>,
	#[cfg(any(test, feature = "test-support"))]
	test_origin: Option<String>,
}
impl TenantSecretBroker {
	pub fn new(
		config: TenantSecretConfig,
		keys: PublicKeys,
		source: Arc<dyn SealedSecretSource>,
		audit: Arc<dyn SecretCallSink>,
	) -> Result<Self, &'static str> {
		if !config.valid() {
			return Err("invalid Tenant Secret broker configuration");
		}
		// Public addresses only, no redirects and no proxy. The resolver refuses
		// a private answer at connect time even if DNS changes after admission.
		let client =
			aidash_integrations::keyless::public_client(config.timeout, Duration::from_secs(3))
				.map_err(|_| "invalid broker HTTP client")?;
		Ok(Self {
			config,
			keys,
			source,
			audit,
			client,
			cache: Arc::default(),
			buckets: Mutex::default(),
			#[cfg(any(test, feature = "test-support"))]
			test_origin: None,
		})
	}
	pub fn router(self: Arc<Self>) -> Router {
		Router::new().fallback(forward).with_state(self)
	}
	/// Replaces the destination origin the broker connects to. This exists only
	/// for integration tests, which run an upstream on loopback; the public
	/// address and sealed-host checks still apply to the token's destination.
	/// The `test-support` feature is not part of any release build.
	#[cfg(any(test, feature = "test-support"))]
	pub fn test_origin(&mut self, origin: String) {
		self.test_origin = Some(origin);
	}
	fn rate_limit(&self, claims: &SecretClaims) -> bool {
		crate::take_token(
			&self.buckets,
			(claims.tenant.clone(), claims.secret),
			self.config.requests_per_second,
			self.config.burst,
		)
	}
	async fn sealed(&self, claims: &SecretClaims) -> Result<CachedSealed, KeyMaterialError> {
		let resource = format!(
			"projects/{}/secrets/{}{}",
			self.config.byok_project_number, self.config.secret_prefix, claims.secret
		);
		let identity = (resource.clone(), claims.version.clone());
		{
			let mut cache = self.cache.lock().unwrap();
			cache.retain(|_, c| c.since.elapsed() < CACHE_TTL);
			if let Some(c) = cache.get(&identity) {
				return Ok(CachedSealed {
					hosts: c.hosts.clone(),
					value: c.value.clone(),
					since: c.since,
				});
			}
		}
		let bytes = self.source.access(&resource, &claims.version).await?;
		let opened = sealed::open(&bytes).map_err(|_| KeyMaterialError::Unavailable)?;
		let since = Instant::now();
		{
			let mut cache = self.cache.lock().unwrap();
			if cache.len() < 10000 {
				cache.insert(
					identity.clone(),
					CachedSealed {
						hosts: opened.hosts.clone(),
						value: opened.value.clone(),
						since,
					},
				);
				// Clear idle entries too. The timer retains neither the broker nor a value.
				let cache = Arc::downgrade(&self.cache);
				tokio::spawn(async move {
					tokio::time::sleep_until(since + CACHE_TTL).await;
					if let Some(cache) = cache.upgrade() {
						let mut cache = cache.lock().expect("Tenant Secret cache lock");
						if cache
							.get(&identity)
							.is_some_and(|entry| entry.since == since)
						{
							cache.remove(&identity);
						}
					}
				});
			}
		}
		Ok(CachedSealed {
			hosts: opened.hosts,
			value: opened.value,
			since,
		})
	}
	fn origin(&self, claims: &SecretClaims) -> String {
		#[cfg(any(test, feature = "test-support"))]
		if let Some(origin) = &self.test_origin {
			return origin.clone();
		}
		claims.destination.origin.clone()
	}
}

async fn forward(State(broker): State<Arc<TenantSecretBroker>>, request: Request) -> Response {
	let started = Instant::now();
	let method = request.method().clone();
	let uri = request.uri().clone();
	let raw_path = uri.path().to_owned();
	let mut guard = AuditGuard::new(
		broker.audit.clone(),
		started,
		SecretCallAudit {
			tenant: String::new(),
			secret: uuid::Uuid::nil(),
			version: String::new(),
			host: String::new(),
			path_sha256: path_digest(&raw_path),
			method: method.as_str().to_owned(),
			status: 499,
			bytes_in: 0,
			bytes_out: 0,
			jti: uuid::Uuid::nil(),
			subject: "unknown".into(),
			duration_ms: 0,
		},
	);
	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();
	let Some(token) = request
		.headers()
		.get(SECRET_CALL)
		.and_then(|value| value.to_str().ok())
	else {
		return guard.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, None);
	};
	let claims =
		match broker
			.keys
			.verify_secret(token, &broker.config.issuers, &broker.config.audience, now)
		{
			Ok(claims) => claims,
			Err(_) => return guard.error(StatusCode::UNAUTHORIZED, BROKER_REJECTED, None),
		};
	guard.entry.jti = claims.jti;
	guard.entry.tenant = claims.tenant.clone();
	guard.entry.secret = claims.secret;
	guard.entry.version = claims.version.clone();
	guard.entry.subject = subject_kind(&claims.sub).into();
	let jti = Some(claims.jti);
	// A name is resolved and checked by `PublicDns` at connect time, but an IP
	// literal never reaches the resolver, so the same domain rule decides it
	// before any connection. The broker permits no private origins.
	if !matches!(
		aidash_domain::keyless::destination(
			&claims.destination.origin,
			&PermittedOrigins::default()
		),
		Ok(Destination::PublicOnly)
	) {
		return guard.error(StatusCode::FORBIDDEN, DESTINATION_REFUSED, jti);
	}
	if !claims
		.methods
		.iter()
		.any(|allowed| allowed == method.as_str())
	{
		return guard.error(StatusCode::FORBIDDEN, DESTINATION_REFUSED, jti);
	}
	let path = match forward_path(&claims, &uri) {
		Ok(path) => path,
		Err(()) => return guard.error(StatusCode::FORBIDDEN, DESTINATION_REFUSED, jti),
	};
	guard.entry.path_sha256 = path_digest(&path);
	if !broker.rate_limit(&claims) {
		return guard.error(StatusCode::TOO_MANY_REQUESTS, RATE_LIMITED, jti);
	}
	let sealed = match broker.sealed(&claims).await {
		Ok(sealed) => sealed,
		Err(KeyMaterialError::Unavailable) => {
			return guard.error(StatusCode::NOT_FOUND, SECRET_UNAVAILABLE, jti);
		}
		Err(KeyMaterialError::StoreUnavailable) => {
			return guard.error(StatusCode::SERVICE_UNAVAILABLE, BROKER_UNAVAILABLE, jti);
		}
	};
	let Some(host) = origin_host(&claims.destination.origin) else {
		return guard.error(StatusCode::FORBIDDEN, DESTINATION_REFUSED, jti);
	};
	guard.entry.host = host.clone();
	if !sealed.hosts.iter().any(|sealed| sealed == &host) {
		return guard.error(StatusCode::FORBIDDEN, DESTINATION_REFUSED, jti);
	}
	let deadline = started + broker.config.timeout;
	let origin = broker.origin(&claims);
	let url = match uri.query() {
		Some(query) => format!("{origin}{path}?{query}"),
		None => format!("{origin}{path}"),
	};
	let headers = request.headers().clone();
	let connection = connection_tokens(&headers);
	let attach = claims.attach.header().to_ascii_lowercase();
	let content_length = headers
		.get(header::CONTENT_LENGTH)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.trim().parse::<u64>().ok());
	// A request without one of the two framing headers has no body, so a GET is
	// never changed into a chunked request. A declared length is forwarded as
	// the exact length of the streamed body.
	let has_body = headers.contains_key(header::TRANSFER_ENCODING)
		|| content_length.is_some_and(|length| length > 0);
	let mut call = broker.client.request(method, url);
	for (name, value) in &headers {
		if forward_header(name.as_str(), &connection, &attach) {
			call = call.header(name, value);
		}
	}
	let attached = match &claims.attach {
		Attach::Bearer => format!("Bearer {}", sealed.value.expose_secret()),
		Attach::Header { .. } => sealed.value.expose_secret().to_owned(),
	};
	let mut value = match HeaderValue::from_str(&attached) {
		Ok(value) => value,
		Err(_) => return guard.error(StatusCode::NOT_FOUND, SECRET_UNAVAILABLE, jti),
	};
	value.set_sensitive(true);
	call = match &claims.attach {
		Attach::Bearer => call.header(header::AUTHORIZATION, value),
		Attach::Header { name } => call.header(name, value),
	};
	let call = if has_body {
		let bytes_in = guard.bytes_in.clone();
		let mut incoming = request.into_body().into_data_stream();
		let body = async_stream::stream! {
			while let Some(chunk) = incoming.next().await {
				match chunk {
					Ok(bytes) => {
						bytes_in.fetch_add(bytes.len() as u64, Ordering::Relaxed);
						yield Ok::<Bytes, std::io::Error>(bytes);
					}
					Err(_) => {
						yield Err(std::io::Error::other("request body unavailable"));
						break;
					}
				}
			}
		};
		let call = call.body(reqwest::Body::wrap_stream(body));
		match content_length.filter(|length| *length > 0) {
			Some(length) => call.header(header::CONTENT_LENGTH, length),
			None => call,
		}
	} else {
		call
	};
	let upstream = match tokio::time::timeout_at(deadline, call.send()).await {
		Ok(Ok(response)) => response,
		Ok(Err(error)) if aidash_integrations::keyless::refused(&error) => {
			return guard.error(StatusCode::FORBIDDEN, DESTINATION_REFUSED, jti);
		}
		_ => return guard.error(StatusCode::BAD_GATEWAY, UPSTREAM_UNREACHABLE, jti),
	};
	forward_response(upstream, guard, deadline, claims.max_response_bytes)
}

fn forward_response(
	upstream: reqwest::Response,
	mut guard: AuditGuard,
	deadline: Instant,
	max_response_bytes: Option<u64>,
) -> Response {
	let status = upstream.status();
	guard.entry.status = status.as_u16();
	let mut builder = Response::builder().status(status);
	let connection = connection_tokens(upstream.headers());
	for (name, value) in upstream.headers() {
		// `x-aidash-*` is the broker's own namespace: a destination must not be
		// able to pass its failure off as the broker's.
		if !response_hop_by_hop(name.as_str())
			&& !name.as_str().starts_with("x-aidash-")
			&& name != header::CONTENT_LENGTH
			&& !connection.iter().any(|token| token == name.as_str())
		{
			builder = builder.header(name, value);
		}
	}
	if upstream.headers().get(header::CACHE_CONTROL).is_none() {
		builder = builder.header(header::CACHE_CONTROL, "no-store");
	}
	let bytes_out = guard.bytes_out.clone();
	let mut stream = upstream.bytes_stream();
	let output: std::pin::Pin<
		Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
	> = Box::pin(async_stream::try_stream! {
		let mut total = 0u64;
		let mut complete = true;
		loop {
			let next = match tokio::time::timeout_at(deadline, stream.next()).await {
				Ok(next) => next,
				Err(_) => {
					complete = false;
					break;
				}
			};
			let Some(chunk) = next else { break; };
			let chunk = match chunk {
				Ok(chunk) => chunk,
				Err(_) => {
					complete = false;
					break;
				}
			};
			let len = chunk.len() as u64;
			if let Some(limit) = max_response_bytes
				&& total.saturating_add(len) > limit
			{
				let remaining = limit.saturating_sub(total) as usize;
				if remaining > 0 {
					bytes_out.fetch_add(remaining as u64, Ordering::Relaxed);
					yield chunk.slice(..remaining);
				}
				tracing::warn!(
					jti = %guard.entry.jti,
					tenant = %guard.entry.tenant,
					secret = %guard.entry.secret,
					limit,
					"Tenant Secret response reached max_response_bytes and the stream was cut"
				);
				return;
			}
			total = total.saturating_add(len);
			bytes_out.fetch_add(len, Ordering::Relaxed);
			yield chunk;
		}
		if !complete {
			tracing::warn!(
				jti = %guard.entry.jti,
				tenant = %guard.entry.tenant,
				secret = %guard.entry.secret,
				"Tenant Secret response ended before upstream completion"
			);
			Err(std::io::Error::other(
				"upstream response unavailable or the call timed out",
			))?;
		}
	});
	builder.body(Body::from_stream(output)).unwrap()
}

fn broker_error(status: StatusCode, code: &'static str, jti: Option<uuid::Uuid>) -> Response {
	let mut builder = Response::builder()
		.status(status)
		.header(header::CONTENT_TYPE, "application/json")
		.header(header::CACHE_CONTROL, "no-store")
		.header(BROKER_ERROR, code);
	if let Some(jti) = jti {
		builder = builder.header(CORRELATION, jti.to_string());
	}
	builder
		.body(Body::from(format!("{{\"error\":\"{code}\"}}")))
		.unwrap()
}

/// The forwarded path: `{rest}` is joined under the signed prefix, with its
/// leading `/` removed. Each segment is percent-decoded for checking only, so
/// a path cannot leave the prefix or hide a second encoding; the original
/// still-encoded path is forwarded.
fn percent_encoding_valid(segment: &str) -> bool {
	let bytes = segment.as_bytes();
	let mut index = 0;
	while index < bytes.len() {
		if bytes[index] == b'%' {
			if index + 2 >= bytes.len()
				|| !bytes[index + 1].is_ascii_hexdigit()
				|| !bytes[index + 2].is_ascii_hexdigit()
			{
				return false;
			}
			index += 3;
		} else {
			index += 1;
		}
	}
	true
}

fn forward_path(claims: &SecretClaims, uri: &Uri) -> Result<String, ()> {
	if uri.scheme().is_some() || uri.authority().is_some() {
		return Err(());
	}
	let path = uri.path();
	let rest = if path == FORWARD {
		""
	} else {
		path.strip_prefix(FORWARD)
			.and_then(|rest| rest.strip_prefix('/'))
			.ok_or(())?
	};
	if rest
		.bytes()
		.any(|b| matches!(b, b'\\' | b'?' | b'#') || b.is_ascii_control())
	{
		return Err(());
	}
	if !rest.is_empty() {
		// A directory-style route keeps its trailing `/`, as the gateway sends
		// it; only that final segment may be empty.
		let segments = rest.strip_suffix('/').unwrap_or(rest);
		if segments.is_empty() {
			return Err(());
		}
		for segment in segments.split('/') {
			if segment.is_empty() || !percent_encoding_valid(segment) {
				return Err(());
			}
			let decoded = percent_decode_str(segment).collect::<Vec<u8>>();
			if decoded.is_empty()
				|| decoded.as_slice() == b"."
				|| decoded.as_slice() == b".."
				|| decoded
					.iter()
					.any(|b| matches!(b, b'/' | b'\\' | b'%') || b.is_ascii_control())
			{
				return Err(());
			}
		}
	}
	let prefix = &claims.destination.path_prefix;
	let joined = format!("{prefix}{rest}");
	if !joined.starts_with(prefix) {
		return Err(());
	}
	Ok(joined)
}

fn origin_host(origin: &str) -> Option<String> {
	let url = url::Url::parse(origin).ok()?;
	if url.scheme() != "https" {
		return None;
	}
	url.host_str().map(str::to_owned)
}

fn subject_kind(subject: &SecretSubject) -> &'static str {
	match subject {
		SecretSubject::Run { .. } => "run",
		SecretSubject::Operation { .. } => "operation",
		SecretSubject::Workbench { .. } => "workbench",
	}
}

fn path_digest(path: &str) -> String {
	let hex: String = Sha256::digest(path.as_bytes())
		.iter()
		.map(|byte| format!("{byte:02x}"))
		.collect();
	hex[..PATH_DIGEST_CHARS].to_owned()
}

fn connection_tokens(headers: &HeaderMap) -> Vec<String> {
	headers
		.get(header::CONNECTION)
		.and_then(|value| value.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|token| token.trim().to_ascii_lowercase())
		.filter(|token| !token.is_empty())
		.collect()
}

fn forward_header(name: &str, connection: &[String], attach: &str) -> bool {
	!matches!(
		name,
		"authorization"
			| "proxy-authorization"
			| "proxy-authenticate"
			| "cookie"
			| "host"
			| "connection"
			| "keep-alive"
			| "proxy-connection"
			| "transfer-encoding"
			| "te"
			| "trailer"
			| "upgrade"
			| "expect"
			// The broker streams the body, so a copied length could be wrong.
			| "content-length"
	) && !name.starts_with("x-aidash-")
		&& !name.starts_with("x-forwarded-")
		&& name != attach
		&& !connection.iter().any(|token| token == name)
}

fn response_hop_by_hop(name: &str) -> bool {
	matches!(
		name,
		"connection"
			| "keep-alive"
			| "proxy-authenticate"
			| "proxy-authorization"
			| "proxy-connection"
			| "te" | "trailer"
			| "transfer-encoding"
			| "upgrade"
	)
}
