//! Stateless Credential Broker: capability admission, fixed Provider Catalog and bounded relay.
use aidash_capability::{
	Claims, Failure, KeyMaterialError, KeyMaterialSource, Operation, PublicKeys,
};
use axum::{
	Router,
	body::{Body, Bytes, to_bytes},
	extract::{Request, State},
	http::{StatusCode, header},
	response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
	time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::time::Instant;

const REQUEST_LIMIT: usize = 1024 * 1024;
// The application's 8 MiB raw-media allowance expands to about 11 MiB in base64.
// Leave bounded room for JSON framing and the approved text context as well.
const CHAT_REQUEST_LIMIT: usize = 16 * 1024 * 1024;
const ERROR_LIMIT: usize = 16 * 1024;
const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Config {
	pub issuer: String,
	pub audience: String,
	/// A canonical numeric project number, also used in IAM resource conditions.
	pub byok_project_number: String,
	pub secret_prefix: String,
	pub requests_per_second: f64,
	pub burst: u32,
	pub inference_deadline: Duration,
}
struct Cached {
	key: SecretString,
	since: Instant,
}
struct Bucket {
	tokens: f64,
	since: Instant,
}
/// Structured audit values contain no request/response body, token or Key Material.
#[derive(Debug, serde::Serialize)]
pub struct Audit {
	pub jti: uuid::Uuid,
	pub subject: aidash_capability::TokenSubject,
	pub tenant: String,
	pub credential: uuid::Uuid,
	pub version: String,
	pub provider: String,
	pub model: String,
	pub operation: Operation,
	pub status: u16,
	pub latency_ms: u128,
	pub usage: Option<Value>,
}
pub trait AuditSink: Send + Sync {
	fn record(&self, audit: &Audit);
}
pub struct CloudLogging;
impl AuditSink for CloudLogging {
	fn record(&self, a: &Audit) {
		// Cloud Run collects this structured JSON stdout event as Cloud Logging.
		tracing::info!(jti = %a.jti, subject = ?a.subject, tenant = %a.tenant, credential = %a.credential, version = %a.version, provider = %a.provider, model = %a.model, operation = ?a.operation, status = a.status, latency_ms = a.latency_ms as u64, usage = ?a.usage, "provider_call");
	}
}
pub struct Broker {
	config: Config,
	keys: PublicKeys,
	source: Arc<dyn KeyMaterialSource>,
	audit: Arc<dyn AuditSink>,
	client: reqwest::Client,
	cache: Arc<Mutex<HashMap<(String, String), Cached>>>,
	buckets: Mutex<HashMap<uuid::Uuid, Bucket>>,
	catalog: HashMap<String, String>,
}
impl Broker {
	pub fn new(
		config: Config,
		keys: PublicKeys,
		source: Arc<dyn KeyMaterialSource>,
		audit: Arc<dyn AuditSink>,
	) -> Result<Self, &'static str> {
		if config.requests_per_second <= 0.0
			|| config.issuer.is_empty()
			|| config.audience.is_empty()
			|| !config.requests_per_second.is_finite()
			// One media inference makes two discovery requests and one chat call.
			|| config.burst < 3
			|| config.inference_deadline.is_zero()
			|| config.inference_deadline > Duration::from_secs(3600)
			|| config.byok_project_number.is_empty()
			|| !config
				.byok_project_number
				.bytes()
				.all(|b| b.is_ascii_digit())
			|| !config.secret_prefix.starts_with("aidash-")
			|| !config.secret_prefix.ends_with("-cred-")
			|| !config
				.secret_prefix
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
		{
			return Err("invalid broker configuration");
		}
		let client = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.connect_timeout(Duration::from_secs(3))
			.timeout(config.inference_deadline)
			.build()
			.map_err(|_| "invalid broker HTTP client")?;
		Ok(Self {
			config,
			keys,
			source,
			audit,
			client,
			cache: Arc::default(),
			buckets: Mutex::default(),
			catalog: HashMap::from([(
				"openrouter".into(),
				aidash_domain::provider_credentials::Provider::Openrouter
					.base_url()
					.into(),
			)]),
		})
	}
	pub fn router(self: Arc<Self>) -> Router {
		Router::new().fallback(relay).with_state(self)
	}
	fn rate_limit(&self, credential: uuid::Uuid) -> bool {
		let now = Instant::now();
		let mut buckets = self.buckets.lock().unwrap();
		// Bound per-instance bookkeeping without keeping per-token state.
		buckets.retain(|_, b| now.duration_since(b.since) < Duration::from_secs(120));
		if buckets.len() >= 10000 && !buckets.contains_key(&credential) {
			return false;
		}
		let b = buckets.entry(credential).or_insert(Bucket {
			tokens: self.config.burst.into(),
			since: now,
		});
		b.tokens = (b.tokens
			+ now.duration_since(b.since).as_secs_f64() * self.config.requests_per_second)
			.min(self.config.burst.into());
		b.since = now;
		if b.tokens < 1.0 {
			return false;
		}
		b.tokens -= 1.0;
		true
	}
	async fn key(&self, claims: &Claims) -> Result<SecretString, KeyMaterialError> {
		let secret = format!(
			"projects/{}/secrets/{}{}",
			self.config.byok_project_number, self.config.secret_prefix, claims.credential
		);
		let identity = (secret.clone(), claims.version.clone());
		{
			let mut cache = self.cache.lock().unwrap();
			cache.retain(|_, c| c.since.elapsed() < CACHE_TTL);
			if let Some(c) = cache.get(&identity) {
				return Ok(c.key.clone());
			}
		}
		let key = self.source.access(&secret, &claims.version).await?;
		let mut cache = self.cache.lock().unwrap();
		if cache.len() < 10000 {
			let since = Instant::now();
			cache.insert(
				identity.clone(),
				Cached {
					key: key.clone(),
					since,
				},
			);
			// Clear idle entries too. The timer retains neither the broker nor Key Material.
			let cache = Arc::downgrade(&self.cache);
			tokio::spawn(async move {
				tokio::time::sleep_until(since + CACHE_TTL).await;
				if let Some(cache) = cache.upgrade() {
					let mut cache = cache.lock().expect("Key Material cache lock");
					if cache
						.get(&identity)
						.is_some_and(|entry| entry.since == since)
					{
						cache.remove(&identity);
					}
				}
			});
		}
		Ok(key)
	}
}
fn failure(f: Failure) -> Response {
	let status = match f {
		Failure::InvalidSignature | Failure::UnknownKid | Failure::Expired | Failure::Audience => {
			StatusCode::UNAUTHORIZED
		}
		_ => StatusCode::FORBIDDEN,
	};
	(
		status,
		axum::Json(json!({"error":{"code":format!("capability_{f}")}})),
	)
		.into_response()
}
fn safe_error(status: StatusCode, reason: &str) -> Response {
	(status, axum::Json(json!({"error":{"message":reason}}))).into_response()
}
fn route(request: &Request, claims: &Claims) -> Result<(Operation, String, usize), Failure> {
	let uri = request.uri();
	if uri.scheme().is_some() || uri.authority().is_some() || uri.query().is_some() {
		return Err(Failure::ClaimViolation);
	}
	let path = uri
		.path()
		.strip_prefix("/api/v1/")
		.ok_or(Failure::Operation)?;
	if path.bytes().any(|b| b == b'%' || b == b'\\')
		|| path
			.split('/')
			.any(|s| s == "." || s == ".." || s.is_empty())
	{
		return Err(Failure::ClaimViolation);
	}
	let result = match (request.method().as_str(), path) {
		("POST", "chat/completions") => (Operation::Chat, 1024 * 1024),
		("POST", "embeddings") => (Operation::Embeddings, 1024 * 1024),
		("GET", "endpoints/zdr") => (Operation::Discovery, 8 * 1024 * 1024),
		("GET", p) if p.starts_with("models/") && p.ends_with("/endpoints") => {
			let model = &p[7..p.len() - 10];
			if model.split('/').count() != 2 || model != claims.model {
				return Err(Failure::Model);
			}
			(Operation::Discovery, 2 * 1024 * 1024)
		}
		_ => return Err(Failure::Operation),
	};
	if !claims.ops.contains(&result.0) {
		return Err(Failure::Operation);
	}
	Ok((result.0, path.into(), result.1))
}
fn body_policy(body: &[u8], claims: &Claims, op: Operation) -> Result<(), Failure> {
	if op == Operation::Discovery {
		return if body.is_empty() {
			Ok(())
		} else {
			Err(Failure::ClaimViolation)
		};
	}
	let value: Value = serde_json::from_slice(body).map_err(|_| Failure::ClaimViolation)?;
	// A capability authorizes one model; OpenRouter's models adds fallback models.
	if value.get("model").and_then(Value::as_str) != Some(&claims.model)
		|| value.get("models").is_some()
	{
		return Err(Failure::Model);
	}
	if op == Operation::Chat
		// Keep one completion limit; the provider also accepts an alternate name.
		&& (value.get("max_completion_tokens").is_some()
			|| value
				.get("max_tokens")
				.and_then(Value::as_u64)
				.is_none_or(|max| max == 0 || max > claims.max_output_tokens))
	{
		return Err(Failure::ClaimViolation);
	}
	if value.pointer("/provider/zdr") != Some(&Value::Bool(true)) {
		return Err(Failure::ClaimViolation);
	}
	Ok(())
}
fn audit(
	claims: &Claims,
	op: Operation,
	status: StatusCode,
	start: Instant,
	usage: Option<Value>,
) -> Audit {
	Audit {
		jti: claims.jti,
		subject: claims.sub.clone(),
		tenant: claims.tenant.clone(),
		credential: claims.credential,
		version: claims.version.clone(),
		provider: claims.provider.clone(),
		model: claims.model.clone(),
		operation: op,
		status: status.as_u16(),
		latency_ms: start.elapsed().as_millis(),
		usage,
	}
}
/// Copy only numeric token usage, never arbitrary upstream fields into logs.
fn usage(value: &Value) -> Option<Value> {
	let v = value.get("usage")?;
	let mut clean = serde_json::Map::new();
	for field in ["prompt_tokens", "completion_tokens", "total_tokens"] {
		if let Some(count) = v.get(field).and_then(Value::as_u64) {
			clean.insert(field.into(), json!(count));
		}
	}
	if clean.is_empty() {
		None
	} else {
		Some(Value::Object(clean))
	}
}
// Drop also records disconnects; the stream owns this guard after headers are sent.
struct AuditGuard {
	started: Instant,
	sink: Arc<dyn AuditSink>,
	entry: Audit,
}
impl Drop for AuditGuard {
	fn drop(&mut self) {
		self.entry.latency_ms = self.started.elapsed().as_millis();
		self.sink.record(&self.entry);
	}
}
impl AuditGuard {
	fn status(&mut self, status: StatusCode) {
		self.entry.status = status.as_u16();
	}
	fn failure(&mut self, value: Failure) -> Response {
		let response = failure(value);
		self.status(response.status());
		response
	}
	fn error(&mut self, status: StatusCode, reason: &str) -> Response {
		self.status(status);
		safe_error(status, reason)
	}
}
async fn relay(State(broker): State<Arc<Broker>>, request: Request) -> Response {
	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();
	let Some(token) = request
		.headers()
		.get(header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Bearer "))
	else {
		return failure(Failure::InvalidSignature);
	};
	let claims =
		match broker
			.keys
			.verify(token, &broker.config.issuer, &broker.config.audience, now)
		{
			Ok(c) => c,
			Err(e) => return failure(e),
		};
	let (op, path, limit) = match route(&request, &claims) {
		Ok(r) => r,
		Err(e) => return failure(e),
	};
	let Some(base) = broker.catalog.get(&claims.provider) else {
		return failure(Failure::Tenant);
	};
	let start = Instant::now();
	let deadline = start + broker.config.inference_deadline;
	let mut guard = AuditGuard {
		started: start,
		sink: broker.audit.clone(),
		entry: audit(&claims, op, StatusCode::from_u16(499).unwrap(), start, None),
	};
	if !broker.rate_limit(claims.credential) {
		return guard.error(
			StatusCode::TOO_MANY_REQUESTS,
			"Provider Credential rate limit exceeded",
		);
	}
	let method = request.method().clone();
	let request_limit = if op == Operation::Chat {
		CHAT_REQUEST_LIMIT
	} else {
		REQUEST_LIMIT
	};
	let body = match tokio::time::timeout_at(deadline, to_bytes(request.into_body(), request_limit))
		.await
	{
		Ok(Ok(b)) => b,
		Ok(Err(_)) => return guard.failure(Failure::ClaimViolation),
		Err(_) => return guard.error(StatusCode::REQUEST_TIMEOUT, "request admission timed out"),
	};
	if let Err(e) = body_policy(&body, &claims, op) {
		return guard.failure(e);
	}
	let key = match tokio::time::timeout_at(deadline, broker.key(&claims)).await {
		Ok(Ok(k)) => k,
		Ok(Err(KeyMaterialError::Unavailable)) => return guard.failure(Failure::Credential),
		_ => {
			return guard.error(
				StatusCode::SERVICE_UNAVAILABLE,
				"Provider Credential Store unavailable",
			);
		}
	};
	// Only the code-defined Provider Catalog chooses the base, never inbound headers/URLs.
	let call = broker
		.client
		.request(method, format!("{}/{path}", base.trim_end_matches('/')))
		.bearer_auth(key.expose_secret())
		.header(header::CONTENT_TYPE, "application/json")
		.body(body);
	let upstream = match tokio::time::timeout_at(deadline, call.send()).await {
		Ok(Ok(r)) => r,
		_ => return guard.error(StatusCode::BAD_GATEWAY, "provider unavailable"),
	};
	let status = upstream.status();
	if !status.is_success() {
		let mut stream = upstream.bytes_stream();
		let mut detail = zeroize::Zeroizing::new(Vec::new());
		while let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(deadline, stream.next()).await {
			if detail.len().saturating_add(chunk.len()) > ERROR_LIMIT {
				break;
			}
			detail.extend_from_slice(&chunk);
		}
		let audio_limit = serde_json::from_slice::<Value>(&detail)
			.ok()
			.and_then(|v| {
				v.pointer("/error/message")
					.and_then(Value::as_str)
					.map(str::to_ascii_lowercase)
			})
			.is_some_and(|v| {
				v.contains("audio")
					&& (v.contains("exceed")
						|| v.contains("too long")
						|| v.contains("duration limit"))
			});
		return guard.error(
			status,
			if audio_limit {
				"Audio exceeds the provider limit"
			} else {
				"upstream rejected the request"
			},
		);
	}
	let sse = upstream
		.headers()
		.get(header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|v| v.starts_with("text/event-stream"));
	let output: std::pin::Pin<
		Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
	> = Box::pin(async_stream::try_stream! {
		let mut stream = upstream.bytes_stream(); let mut total = 0usize; let mut pending = Vec::new();
		let mut completed = true;
		loop {
			let next = match tokio::time::timeout_at(deadline, stream.next()).await { Ok(v) => v, Err(_) => { completed = false; break; } };
			let Some(chunk) = next else { break; };
			let chunk = match chunk { Ok(c) => c, Err(_) => { completed = false; break; } };
			total = total.saturating_add(chunk.len()); if total > limit { completed = false; break; }
			if sse {
				// Keep at most one bounded SSE line; streamed bodies never accumulate.
				for byte in &chunk {
					if *byte == b'\n' {
						if let Some(data) = pending.strip_prefix(b"data:") && let Ok(value) = serde_json::from_slice::<Value>(data)
							&& let Some(u) = usage(&value) { guard.entry.usage = Some(u); }
						pending.clear();
					} else if pending.len() <= ERROR_LIMIT { pending.push(*byte); }
				}
			} else { pending.extend_from_slice(&chunk); }
			yield chunk;
		}
		if !sse && let Ok(value) = serde_json::from_slice::<Value>(&pending) { guard.entry.usage = usage(&value); }
		guard.status(if completed { status } else { StatusCode::BAD_GATEWAY });
		if !completed { Err(std::io::Error::other("provider response unavailable or limit exceeded"))?; }
	});
	Response::builder()
		.status(status)
		.header(
			header::CONTENT_TYPE,
			if sse {
				"text/event-stream"
			} else {
				"application/json"
			},
		)
		.header(header::CACHE_CONTROL, "no-store")
		.body(Body::from_stream(output))
		.unwrap()
}

#[cfg(test)]
mod tests;
