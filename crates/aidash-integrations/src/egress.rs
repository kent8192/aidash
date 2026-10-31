//! Internal HTTP streaming adapter. The dashboard framework buffers incoming
//! bodies, so this listener uses Hyper directly; it has no public API routes.
use aidash_application::egress::{Admission, Authority, Direction, Meter};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{Request, Response, StatusCode, body::Frame, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};
use std::{
	convert::Infallible,
	sync::Arc,
	time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::watch};
use url::Url;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Body = UnsyncBoxBody<Bytes, BoxError>;
const IDLE: Duration = Duration::from_secs(30);
const INDEX_BYTES: usize = 8 * 1024 * 1024;
const BROKER_ERROR: &str = "x-aidash-broker-error";
const SECRET_CALL: &str = "x-aidash-secret-call";

fn failure(status: StatusCode, code: &'static str) -> Response<Body> {
	Response::builder()
		.status(status)
		.header("content-type", "application/json")
		.header("cache-control", "no-store")
		.body(
			Full::new(Bytes::from(format!("{{\"error\":\"{code}\"}}")))
				.map_err(|never| match never {})
				.boxed_unsync(),
		)
		.unwrap()
}
/// A broker-owned error never carries a value to the sandbox: the gateway
/// reports the code alone (ADR 0095).
fn broker_failure(code: &str) -> (StatusCode, &'static str) {
	match code {
		"secret_unavailable" => (StatusCode::FAILED_DEPENDENCY, "secret_unavailable"),
		"secret_destination_refused" => (StatusCode::FORBIDDEN, "secret_destination_refused"),
		"rate_limited" => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
		"upstream_unreachable" => (StatusCode::BAD_GATEWAY, "upstream_unreachable"),
		"broker_rejected" => (StatusCode::BAD_GATEWAY, "broker_rejected"),
		_ => (StatusCode::SERVICE_UNAVAILABLE, "broker_unavailable"),
	}
}
/// The request's path under the service base's directory, appended to the broker's
/// `/v1/forward` base. The broker joins it to the signed prefix in turn.
fn broker_url(
	route: &aidash_application::tenant_secrets::SecretRoute,
	service: &Url,
	base: &Url,
) -> Result<Url, BoxError> {
	// The broker signs the base path's directory, so an endpoint-style base
	// such as `/invoke` forwards `invoke` exactly rather than `invoke/`.
	let prefix = aidash_application::tenant_secrets::path_directory(base.path());
	let relative = service
		.path()
		.strip_prefix(prefix)
		.ok_or_else(|| io_error("invalid_egress_path"))?;
	let mut target = route.base.as_str().trim_end_matches('/').to_owned();
	if !relative.is_empty() {
		target.push('/');
		target.push_str(relative);
	}
	if let Some(query) = service.query() {
		target.push('?');
		target.push_str(query);
	}
	Url::parse(&target).map_err(Into::into)
}
fn io_error(code: &'static str) -> BoxError {
	std::io::Error::other(code).into()
}

async fn charge(
	meter: &dyn Meter,
	direction: Direction,
	bytes: usize,
	deadline: tokio::time::Instant,
) -> Result<(), BoxError> {
	tokio::time::timeout_at(deadline, meter.charge(direction, bytes))
		.await
		.map_err(|_| io_error("operation_expired"))?
		.map_err(|_| io_error("egress_limit_exceeded"))
}

/// Each stream, including a partially consumed one, completes its audit record.
struct Completion {
	meter: Arc<dyn Meter>,
	started: Instant,
	status: u16,
	finished: bool,
}
impl Completion {
	async fn finish(&mut self, error: Option<&str>) {
		if self.finished {
			return;
		}
		self.finished = true;
		if self
			.meter
			.finish(
				self.status,
				self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
				error,
			)
			.await
			.is_err()
		{
			tracing::error!("Egress request audit could not be completed");
		}
	}
}
impl Drop for Completion {
	fn drop(&mut self) {
		if !self.finished {
			let meter = self.meter.clone();
			let status = self.status;
			let elapsed = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
			tokio::spawn(async move {
				let _ = meter
					.finish(status, elapsed, Some("stream_interrupted"))
					.await;
			});
		}
	}
}

/// The listener must be bound only on a cluster-internal interface. Its token
/// is an operation credential, never a Tenant Secret or a Deployment secret.
pub async fn serve(
	listener: TcpListener,
	authority: Arc<dyn Authority>,
	mut stopping: watch::Receiver<bool>,
) -> crate::Result<()> {
	let client = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.no_proxy()
		.connect_timeout(IDLE)
		.read_timeout(IDLE)
		.build()
		.map_err(crate::http_error)?;
	let public = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.no_proxy()
		.dns_resolver(Arc::new(crate::keyless::PublicDns))
		.connect_timeout(IDLE)
		.read_timeout(IDLE)
		.build()
		.map_err(crate::http_error)?;
	let slots = Arc::new(tokio::sync::Semaphore::new(128));
	let mut connections = tokio::task::JoinSet::new();
	loop {
		if *stopping.borrow() {
			connections.abort_all();
			return Ok(());
		}
		tokio::select! {
			_ = stopping.changed() => {},
			Some(_) = connections.join_next(), if !connections.is_empty() => {},
			accepted = listener.accept() => {
				let (socket, _) = accepted.map_err(|_| aidash_application::Error::External("Egress listener unavailable".into()))?;
				let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
				let authority = authority.clone(); let client = client.clone(); let public = public.clone();
				connections.spawn(async move {
					let _permit = permit;
					let service = service_fn(move |request| {
						let authority=authority.clone(); let client=client.clone(); let public=public.clone();
						async move { Ok::<_,Infallible>(forward(request, authority, client, public).await) }
					});
					let _ = hyper::server::conn::http1::Builder::new().timer(TokioTimer::new())
						.header_read_timeout(IDLE).max_headers(64)
						.serve_connection(TokioIo::new(socket), service).await;
				});
			}
		}
	}
}

fn target(admission: &Admission, path: &str, query: Option<&str>) -> Result<Url, BoxError> {
	let base = Url::parse(&admission.definition.base_url)?;
	let mut url = base.join(path.trim_start_matches('/'))?;
	if url.origin() != base.origin()
		|| !url.path().starts_with(base.path())
		|| !url.username().is_empty()
		|| url.password().is_some()
	{
		return Err(io_error("invalid_egress_path"));
	}
	url.set_query(query);
	url.set_fragment(None);
	Ok(url)
}

fn forwarded_header(name: &str, connection: &[String]) -> bool {
	!matches!(
		name,
		"authorization"
			| "proxy-authorization"
			| "proxy-authenticate"
			| "cookie"
			| "set-cookie"
			| "host" | "connection"
			| "keep-alive"
			| "transfer-encoding"
			| "te" | "trailer"
			| "upgrade"
			| "expect"
			| "origin"
			| "referer"
	) && !name.starts_with("x-forwarded-")
		&& !connection.iter().any(|v| v == name)
}

fn package_url(service: &str, url: &Url) -> bool {
	url.scheme() == "https"
		&& url.username().is_empty()
		&& url.password().is_none()
		&& url.port_or_known_default() == Some(443)
		&& matches!(
			(service, url.host_str()),
			("core.npm", Some("registry.npmjs.org"))
				| ("core.pypi", Some("pypi.org" | "files.pythonhosted.org"))
		)
}
fn package_link(
	value: &str,
	source: &Url,
	origin: &str,
	token: &str,
	service: &str,
) -> Option<String> {
	let url = source.join(value).ok()?;
	if !package_url(service, &url) {
		return None;
	}
	let mut url = url;
	let fragment = url.fragment().map(str::to_owned);
	url.set_fragment(None);
	Some(format!(
		"{}/v1/{token}/{service}/__asset/{}{}",
		origin.trim_end_matches('/'),
		URL_SAFE_NO_PAD.encode(url.as_str()),
		fragment.map(|f| format!("#{f}")).unwrap_or_default()
	))
}
fn rewrite_json(
	bytes: &[u8],
	source: &Url,
	origin: &str,
	token: &str,
	service: &str,
) -> Result<Vec<u8>, BoxError> {
	fn visit(
		value: &mut serde_json::Value,
		source: &Url,
		origin: &str,
		token: &str,
		service: &str,
	) {
		match value {
			serde_json::Value::Array(items) => {
				for item in items {
					visit(item, source, origin, token, service);
				}
			}
			serde_json::Value::Object(items) => {
				for (key, value) in items {
					if matches!(key.as_str(), "tarball" | "url" | "href")
						&& let Some(link) = value
							.as_str()
							.and_then(|v| package_link(v, source, origin, token, service))
					{
						*value = link.into();
					} else {
						visit(value, source, origin, token, service);
					}
				}
			}
			_ => {}
		}
	}
	let mut value = serde_json::from_slice(bytes)?;
	visit(&mut value, source, origin, token, service);
	Ok(serde_json::to_vec(&value)?)
}
fn rewrite_html(
	bytes: &[u8],
	source: &Url,
	origin: &str,
	token: &str,
	service: &str,
) -> Result<Vec<u8>, BoxError> {
	let text = std::str::from_utf8(bytes)?;
	let mut result = String::with_capacity(text.len());
	let mut rest = text;
	// PyPI's Simple API uses quoted href attributes. Unknown or foreign links
	// remain unreachable under the Pod's NetworkPolicy.
	while let Some(start) = rest.find("href=") {
		result.push_str(&rest[..start + 5]);
		rest = &rest[start + 5..];
		let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
			continue;
		};
		result.push(quote);
		rest = &rest[quote.len_utf8()..];
		let Some(end) = rest.find(quote) else {
			return Err(io_error("invalid_package_index"));
		};
		let value = &rest[..end];
		let decoded = value.replace("&amp;", "&");
		result.push_str(
			&package_link(&decoded, source, origin, token, service)
				.unwrap_or_else(|| value.to_owned()),
		);
		result.push(quote);
		rest = &rest[end + quote.len_utf8()..];
	}
	result.push_str(rest);
	Ok(result.into_bytes())
}

async fn forward(
	request: Request<hyper::body::Incoming>,
	authority: Arc<dyn Authority>,
	client: reqwest::Client,
	public: reqwest::Client,
) -> Response<Body> {
	if !request.uri().path().starts_with("/v1/") || request.uri().scheme().is_some() {
		return failure(StatusCode::NOT_FOUND, "egress_route_not_found");
	}
	let route = request.uri().path()[4..].to_owned();
	let Some((token, remainder)) = route.split_once('/') else {
		return failure(StatusCode::UNAUTHORIZED, "egress_token_required");
	};
	let (service, path) = remainder.split_once('/').unwrap_or((remainder, ""));
	if token.len() > 512
		|| service.is_empty()
		|| service.len() > 100
		|| !service
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
	{
		return failure(StatusCode::BAD_REQUEST, "invalid_egress_route");
	}
	let asset = if let Some(encoded) = path.strip_prefix("__asset/") {
		let decoded = URL_SAFE_NO_PAD
			.decode(encoded)
			.ok()
			.and_then(|b| String::from_utf8(b).ok())
			.and_then(|s| Url::parse(&s).ok());
		let Some(url) = decoded.filter(|u| package_url(service, u)) else {
			return failure(StatusCode::BAD_REQUEST, "invalid_package_url");
		};
		Some(url)
	} else {
		None
	};
	let query = asset
		.as_ref()
		.and_then(Url::query)
		.or_else(|| request.uri().query())
		.map(str::to_owned);
	let audit_path = asset.as_ref().map(Url::path).unwrap_or(path);
	let head = request.method() == hyper::Method::HEAD;
	let query_digest = format!(
		"{:x}",
		Sha256::digest(query.as_deref().unwrap_or("").as_bytes())
	);
	let admission = match authority
		.admit(
			token,
			service,
			request.method().as_str(),
			audit_path,
			&query_digest,
		)
		.await
	{
		Ok(value) => value,
		Err(aidash_application::Error::Forbidden | aidash_application::Error::Unauthorized) => {
			return failure(StatusCode::FORBIDDEN, "egress_forbidden");
		}
		Err(aidash_application::Error::Conflict(_)) => {
			return failure(StatusCode::TOO_MANY_REQUESTS, "egress_limit_exceeded");
		}
		Err(aidash_application::Error::Invalid(message)) if message == "secret_unavailable" => {
			return failure(StatusCode::FAILED_DEPENDENCY, "secret_unavailable");
		}
		Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE, "EGRESS_UNAVAILABLE"),
	};
	let remaining = (admission.expires_at - chrono::Utc::now())
		.to_std()
		.unwrap_or_default();
	let deadline = tokio::time::Instant::now() + remaining;
	let mut completion = Completion {
		meter: admission.meter.clone(),
		started: Instant::now(),
		status: 502,
		finished: false,
	};
	if remaining.is_zero() {
		completion.finish(Some("operation_expired")).await;
		return failure(StatusCode::FORBIDDEN, "egress_expired");
	}
	let index = admission.definition.builtin_index(service);
	if asset.is_some() && !index {
		completion.finish(Some("invalid_package_service")).await;
		return failure(StatusCode::FORBIDDEN, "egress_forbidden");
	}
	let service_url = match asset
		.clone()
		.map(Ok)
		.unwrap_or_else(|| target(&admission, path, query.as_deref()))
	{
		Ok(url) => url,
		Err(_) => {
			completion.finish(Some("invalid_path")).await;
			return failure(StatusCode::BAD_REQUEST, "invalid_egress_path");
		}
	};
	// A brokered request goes to the broker's `/v1/forward` base instead of the
	// External service. The broker attaches the Tenant Secret (ADR 0095).
	let brokered = admission.route.is_some();
	let upstream_url = match &admission.route {
		Some(route) => {
			let base = Url::parse(&admission.definition.base_url).expect("admitted service URL");
			match broker_url(route, &service_url, &base) {
				Ok(url) => url,
				Err(_) => {
					completion.finish(Some("invalid_path")).await;
					return failure(StatusCode::BAD_REQUEST, "invalid_egress_path");
				}
			}
		}
		None => service_url.clone(),
	};
	let connection = request
		.headers()
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	// The broker is an in-cluster service: it resolves the External service
	// itself and applies the public-address rule there.
	let selected = if brokered {
		&client
	} else {
		match admission.destination {
			aidash_domain::keyless::Destination::PublicOnly => &public,
			aidash_domain::keyless::Destination::Permitted => &client,
			aidash_domain::keyless::Destination::Forbidden => {
				completion.finish(Some("destination_forbidden")).await;
				return failure(StatusCode::FORBIDDEN, "egress_forbidden");
			}
		}
	};
	let mut upstream = selected.request(request.method().clone(), upstream_url);
	for (name, value) in request.headers() {
		if forwarded_header(name.as_str(), &connection)
			&& name.as_str()
				!= admission
					.definition
					.authentication
					.header()
					.to_ascii_lowercase()
			// The broker owns this header; an inbound one must never override
			// the token the gateway mints.
			&& !(brokered && name.as_str() == SECRET_CALL)
		{
			upstream = upstream.header(name, value);
		}
	}
	if let Some(route) = &admission.route {
		let Ok(mut value) = hyper::header::HeaderValue::from_str(route.token.expose_secret())
		else {
			completion.status = StatusCode::SERVICE_UNAVAILABLE.as_u16();
			completion.finish(Some("broker_unavailable")).await;
			return failure(StatusCode::SERVICE_UNAVAILABLE, "broker_unavailable");
		};
		value.set_sensitive(true);
		upstream = upstream.header(SECRET_CALL, value);
	} else if let Some(secret) = &admission.credential {
		upstream = match &admission.definition.authentication {
			aidash_domain::egress::Authentication::Bearer => {
				upstream.bearer_auth(secret.expose_secret())
			}
			aidash_domain::egress::Authentication::Header { name } => {
				let Ok(mut value) = hyper::header::HeaderValue::from_str(secret.expose_secret())
				else {
					completion.finish(Some("secret_unavailable")).await;
					return failure(StatusCode::FAILED_DEPENDENCY, "secret_unavailable");
				};
				value.set_sensitive(true);
				upstream.header(name, value)
			}
		};
	}
	if index && asset.is_none() {
		upstream = upstream.header("accept-encoding", "identity");
	}
	let meter = admission.meter.clone();
	let mut incoming = request.into_body().into_data_stream();
	let body = async_stream::try_stream! {
		loop {
			let frame = tokio::time::timeout_at(deadline, tokio::time::timeout(IDLE,incoming.next())).await
				.map_err(|_|io_error("operation_expired"))?.map_err(|_|io_error("egress_idle_timeout"))?;
			let Some(frame) = frame else { break; };
			let bytes = frame.map_err(|_|io_error("egress_upload_interrupted"))?;
			charge(meter.as_ref(), Direction::Sent, bytes.len(), deadline).await?;
			yield bytes;
		}
	};
	let body = body.map_err(|error: BoxError| error);
	let response = match tokio::time::timeout_at(
		deadline,
		upstream.body(reqwest::Body::wrap_stream(body)).send(),
	)
	.await
	{
		Ok(Ok(response)) => response,
		_ => {
			completion.finish(Some("upstream_unavailable")).await;
			return failure(StatusCode::BAD_GATEWAY, "EGRESS_UNAVAILABLE");
		}
	};
	completion.status = response.status().as_u16();
	// A broker-owned error is returned as a gateway error that names the code
	// only; an upstream status the broker passes through stays as it is.
	if brokered
		&& let Some(code) = response
			.headers()
			.get(BROKER_ERROR)
			.and_then(|value| value.to_str().ok())
	{
		let (status, code) = broker_failure(code);
		completion.status = status.as_u16();
		completion.finish(Some(code)).await;
		return failure(status, code);
	}
	let mut headers = response.headers().clone();
	let connection = headers
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	for name in headers.keys().cloned().collect::<Vec<_>>() {
		if !forwarded_header(name.as_str(), &connection)
			|| name.as_str()
				== admission
					.definition
					.authentication
					.header()
					.to_ascii_lowercase()
		{
			headers.remove(name);
		}
	}
	// A redirect is followed through the gateway, with a fresh live decision and
	// another request debit. Streaming uploads are never buffered for replay.
	if response.status().is_redirection()
		&& let Some(location) = headers.get("location")
	{
		let redirected = location
			.to_str()
			.ok()
			.and_then(|s| service_url.join(s).ok());
		let base = Url::parse(&admission.definition.base_url).expect("admitted service URL");
		let Some(redirected) = redirected.filter(|u| {
			u.origin() == base.origin()
				&& u.path().starts_with(base.path())
				&& u.username().is_empty()
				&& u.password().is_none()
		}) else {
			completion.finish(Some("redirect_refused")).await;
			return failure(StatusCode::BAD_GATEWAY, "egress_redirect_refused");
		};
		let relative = redirected.path().strip_prefix(base.path()).unwrap_or("");
		let rewritten = format!(
			"/v1/{token}/{service}/{relative}{}",
			redirected
				.query()
				.map(|q| format!("?{q}"))
				.unwrap_or_default()
		);
		let Ok(location) = rewritten.parse() else {
			completion.finish(Some("redirect_refused")).await;
			return failure(StatusCode::BAD_GATEWAY, "egress_redirect_refused");
		};
		headers.insert("location", location);
	}
	headers.insert("cache-control", "no-store".parse().unwrap());
	let status = response.status();
	// Hyper does not poll bodies for these responses. Complete their audit now,
	// rather than classifying a correctly empty response as an interruption.
	if head || matches!(status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED) {
		completion.finish(None).await;
		let mut result = Response::new(
			Full::new(Bytes::new())
				.map_err(|never| match never {})
				.boxed_unsync(),
		);
		*result.status_mut() = status;
		*result.headers_mut() = headers;
		return result;
	}
	let index_type = response
		.headers()
		.get("content-type")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.to_owned();
	let mut downstream = response.bytes_stream();
	if index
		&& asset.is_none()
		&& !head
		&& (index_type.contains("json") || index_type.starts_with("text/html"))
	{
		let mut buffer = Vec::new();
		loop {
			let next =
				tokio::time::timeout_at(deadline, tokio::time::timeout(IDLE, downstream.next()))
					.await;
			let bytes = match next {
				Ok(Ok(Some(Ok(bytes)))) => bytes,
				Ok(Ok(None)) => break,
				_ => {
					completion.finish(Some("index_unavailable")).await;
					return failure(StatusCode::BAD_GATEWAY, "package_index_unavailable");
				}
			};
			if charge(
				admission.meter.as_ref(),
				Direction::Received,
				bytes.len(),
				deadline,
			)
			.await
			.is_err() || buffer.len().saturating_add(bytes.len()) > INDEX_BYTES
			{
				completion.finish(Some("index_limit_exceeded")).await;
				return failure(StatusCode::BAD_GATEWAY, "package_index_limit_exceeded");
			}
			buffer.extend_from_slice(&bytes);
		}
		let rewritten = if index_type.contains("json") {
			rewrite_json(
				&buffer,
				&service_url,
				&admission.gateway_origin,
				token,
				service,
			)
		} else {
			rewrite_html(
				&buffer,
				&service_url,
				&admission.gateway_origin,
				token,
				service,
			)
		};
		let Ok(rewritten) = rewritten else {
			completion.finish(Some("invalid_package_index")).await;
			return failure(StatusCode::BAD_GATEWAY, "invalid_package_index");
		};
		if rewritten.len() > buffer.len()
			&& charge(
				admission.meter.as_ref(),
				Direction::Received,
				rewritten.len() - buffer.len(),
				deadline,
			)
			.await
			.is_err()
		{
			completion.finish(Some("egress_limit_exceeded")).await;
			return failure(StatusCode::TOO_MANY_REQUESTS, "egress_limit_exceeded");
		}
		completion.finish(None).await;
		headers.remove("content-length");
		headers.remove("content-encoding");
		headers.remove("etag");
		headers.remove("content-md5");
		let mut result = Response::new(
			Full::new(Bytes::from(rewritten))
				.map_err(|never| match never {})
				.boxed_unsync(),
		);
		*result.status_mut() = status;
		*result.headers_mut() = headers;
		return result;
	}
	let meter = admission.meter.clone();
	let output = async_stream::try_stream! {
		loop {
			let next = match tokio::time::timeout_at(deadline,tokio::time::timeout(IDLE,downstream.next())).await {
				Ok(Ok(next)) => next,
				_ => { completion.finish(Some("egress_timeout")).await; Err(io_error("egress_timeout"))?; unreachable!() }
			};
			let Some(bytes) = next else { completion.finish(None).await; break; };
			let bytes=match bytes { Ok(bytes)=>bytes, Err(_)=>{ completion.finish(Some("upstream_interrupted")).await; Err(io_error("upstream_interrupted"))?; unreachable!() } };
			if charge(meter.as_ref(), Direction::Received, bytes.len(), deadline).await.is_err() {
				completion.finish(Some("egress_limit_exceeded")).await; Err(io_error("egress_limit_exceeded"))?;
			}
			yield Frame::data(bytes);
		}
	};
	let output = output.map_err(|error: BoxError| error);
	let mut result = Response::new(StreamBody::new(output).boxed_unsync());
	*result.status_mut() = status;
	*result.headers_mut() = headers;
	result
}

#[cfg(test)]
mod tests;
