//! Process-local HTTP admission, safe request logging and metrics.
use crate::authorization::identity::Actor;
use axum::{
	Router,
	body::Body,
	error_handling::HandleErrorLayer,
	extract::{DefaultBodyLimit, MatchedPath, Request},
	http::{HeaderName, HeaderValue, StatusCode, header},
	middleware::{self, Next},
	response::{IntoResponse, Response},
};
use std::{
	net::IpAddr,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	time::Duration,
};
use tower::{ServiceBuilder, limit::GlobalConcurrencyLimitLayer, load_shed::LoadShedLayer};
use tower_governor::{
	GovernorError, GovernorLayer, governor::GovernorConfigBuilder, key_extractor::KeyExtractor,
};
use tower_http::{
	limit::RequestBodyLimitLayer,
	request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
	sensitive_headers::{SetSensitiveRequestHeadersLayer, SetSensitiveResponseHeadersLayer},
	timeout::TimeoutLayer,
	trace::TraceLayer,
};

pub const BODY_LIMIT: usize = 1024 * 1024;

/// Apply before merging route groups with different payload limits.
pub(crate) fn body_limit(limit: usize) -> (DefaultBodyLimit, RequestBodyLimitLayer) {
	(
		DefaultBodyLimit::max(limit),
		RequestBodyLimitLayer::new(limit),
	)
}

#[derive(Clone, Debug)]
pub struct Settings {
	pub timeout: Duration,
	pub concurrency: usize,
	pub sse_connections: usize,
	pub auth_burst: u32,
	pub auth_period: Duration,
	/// Exact socket peers allowed to supply a sanitized, single X-Real-IP.
	pub auth_trusted_proxy_ips: Vec<IpAddr>,
	pub actor_burst: u32,
	pub actor_period: Duration,
	pub peer_burst: u32,
	pub peer_period: Duration,
}
impl Default for Settings {
	fn default() -> Self {
		Self {
			timeout: Duration::from_secs(30),
			concurrency: 128,
			sse_connections: 128,
			auth_burst: 30,
			auth_period: Duration::from_secs(2),
			auth_trusted_proxy_ips: Vec::new(),
			actor_burst: 120,
			actor_period: Duration::from_millis(100),
			peer_burst: 240,
			peer_period: Duration::from_millis(50),
		}
	}
}
impl Settings {
	pub fn from_env() -> crate::Result<Self> {
		fn value(name: &str, default: u64, maximum: u64) -> crate::Result<u64> {
			let value = match std::env::var(name) {
				Ok(raw) => raw
					.parse()
					.map_err(|_| crate::Error::Invalid(format!("invalid {name}")))?,
				Err(std::env::VarError::NotPresent) => default,
				Err(_) => return Err(crate::Error::Invalid(format!("invalid {name}"))),
			};
			if value == 0 || value > maximum {
				return Err(crate::Error::Invalid(format!(
					"{name} must be 1..={maximum}"
				)));
			}
			Ok(value)
		}
		let defaults = Self::default();
		Ok(Self {
			auth_trusted_proxy_ips: match std::env::var("AIDASH_AUTH_TRUSTED_PROXY_IPS") {
				Ok(raw) if raw.trim().is_empty() => Vec::new(),
				Ok(raw) => raw
					.split(',')
					.map(|ip| {
						ip.trim().parse().map_err(|_| {
							crate::Error::Invalid("invalid AIDASH_AUTH_TRUSTED_PROXY_IPS".into())
						})
					})
					.collect::<crate::Result<Vec<_>>>()?,
				Err(std::env::VarError::NotPresent) => Vec::new(),
				Err(_) => {
					return Err(crate::Error::Invalid(
						"invalid AIDASH_AUTH_TRUSTED_PROXY_IPS".into(),
					));
				}
			},
			auth_period: Duration::from_millis(value(
				"AIDASH_AUTH_RATE_PERIOD_MS",
				defaults.auth_period.as_millis() as u64,
				3600000,
			)?),
			actor_period: Duration::from_millis(value(
				"AIDASH_API_RATE_PERIOD_MS",
				defaults.actor_period.as_millis() as u64,
				3600000,
			)?),
			peer_period: Duration::from_millis(value(
				"AIDASH_PEER_RATE_PERIOD_MS",
				defaults.peer_period.as_millis() as u64,
				3600000,
			)?),
			timeout: Duration::from_secs(value(
				"AIDASH_HTTP_TIMEOUT_SECONDS",
				defaults.timeout.as_secs(),
				3600,
			)?),
			concurrency: value(
				"AIDASH_HTTP_CONCURRENCY",
				defaults.concurrency as u64,
				65536,
			)? as usize,
			sse_connections: value(
				"AIDASH_SSE_CONNECTIONS",
				defaults.sse_connections as u64,
				65536,
			)? as usize,
			auth_burst: value("AIDASH_AUTH_RATE_BURST", defaults.auth_burst as u64, 100000)? as u32,
			actor_burst: value("AIDASH_API_RATE_BURST", defaults.actor_burst as u64, 100000)?
				as u32,
			peer_burst: value("AIDASH_PEER_RATE_BURST", defaults.peer_burst as u64, 100000)? as u32,
		})
	}
}

/// Only constructed after federation authentication succeeds.
#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedPeer(pub String);
#[derive(Clone)]
pub(crate) struct AuthKey(pub Vec<IpAddr>);
impl KeyExtractor for AuthKey {
	type Key = IpAddr;
	fn extract<T>(&self, request: &axum::http::Request<T>) -> Result<IpAddr, GovernorError> {
		let peer = tower_governor::key_extractor::PeerIpKeyExtractor.extract(request)?;
		if self.0.contains(&peer) {
			let mut values = request.headers().get_all("x-real-ip").iter();
			let client = values
				.next()
				.and_then(|value| value.to_str().ok())
				.and_then(|value| value.parse::<IpAddr>().ok());
			if values.next().is_none()
				&& let Some(client) = client
			{
				return Ok(client);
			}
		}
		Ok(peer)
	}
}
#[derive(Clone)]
pub(crate) struct ActorKey;
impl KeyExtractor for ActorKey {
	type Key = (String, String);
	fn extract<T>(&self, request: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
		match request.extensions().get::<Actor>() {
			Some(Actor::Operator) => Ok(("operator".into(), String::new())),
			Some(Actor::Subject(identity)) => {
				Ok((identity.tenant.clone(), identity.subject.clone()))
			}
			None => Err(GovernorError::UnableToExtractKey),
		}
	}
}
#[derive(Clone)]
pub(crate) struct PeerKey;
impl KeyExtractor for PeerKey {
	type Key = String;
	fn extract<T>(&self, request: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
		request
			.extensions()
			.get::<AuthenticatedPeer>()
			.map(|peer| peer.0.clone())
			.ok_or(GovernorError::UnableToExtractKey)
	}
}

pub(crate) fn rate_limit<S, K>(router: Router<S>, key: K, burst: u32, period: Duration) -> Router<S>
where
	S: Clone + Send + Sync + 'static,
	K: KeyExtractor + Send + Sync + 'static,
	K::Key: Send + Sync,
{
	let config = GovernorConfigBuilder::default()
		.key_extractor(key)
		.period(period)
		.burst_size(burst)
		.finish()
		.expect("positive rate limits");
	let limiter = config.limiter().clone();
	let requests = Arc::new(AtomicUsize::new(0));
	router
		.layer(GovernorLayer::new(config))
		.layer(middleware::from_fn(move |request: Request, next: Next| {
			let limiter = limiter.clone();
			let requests = requests.clone();
			async move {
				// Bound stale key retention without a background task outliving the router.
				if requests
					.fetch_add(1, Ordering::Relaxed)
					.is_multiple_of(1024)
				{
					limiter.retain_recent();
				}
				next.run(request).await
			}
		}))
}

/// All clones and routes share the same semaphore; rejected requests never queue.
pub(crate) fn protect<S>(router: Router<S>, settings: &Settings) -> Router<S>
where
	S: Clone + Send + Sync + 'static,
{
	let request_id = HeaderName::from_static("x-request-id");
	let trace = TraceLayer::new_for_http()
        .make_span_with(|request: &Request| {
            tracing::info_span!("http.request",
                request_id = request.headers().get("x-request-id").and_then(|v| v.to_str().ok()).unwrap_or("unknown"),
                method = %request.method(),
                route = request.extensions().get::<MatchedPath>().map(MatchedPath::as_str).unwrap_or("unmatched"))
        })
        .on_request(())
        .on_response(|response: &Response<_>, latency: Duration, span: &tracing::Span| {
            tracing::info!(parent: span, status = response.status().as_u16(), duration_ms = latency.as_secs_f64() * 1000.0, "HTTP response");
        })
        .on_failure(());
	let metrics = axum_prometheus::PrometheusMetricLayerBuilder::new()
		.with_endpoint_label_type(axum_prometheus::EndpointLabel::MatchedPathWithFallbackFn(
			|_| "unmatched".into(),
		))
		.build();
	router
		.layer(
			ServiceBuilder::new()
				.layer(SetRequestIdLayer::new(request_id.clone(), MakeRequestUuid))
				.layer(SetSensitiveRequestHeadersLayer::new([
					header::AUTHORIZATION,
					header::COOKIE,
					HeaderName::from_static("x-aidash-csrf"),
				]))
				.layer(trace)
				.layer(metrics)
				.layer(PropagateRequestIdLayer::new(request_id))
				.layer(SetSensitiveResponseHeadersLayer::new([header::SET_COOKIE]))
				.layer(
					tower_http::set_header::SetResponseHeaderLayer::if_not_present(
						header::X_CONTENT_TYPE_OPTIONS,
						HeaderValue::from_static("nosniff"),
					),
				)
				.layer(HandleErrorLayer::new(|_: tower::BoxError| async {
					(
						StatusCode::SERVICE_UNAVAILABLE,
						[(header::RETRY_AFTER, "1")],
						"server overloaded",
					)
				}))
				.layer(LoadShedLayer::new())
				.layer(GlobalConcurrencyLimitLayer::new(settings.concurrency))
				.layer(TimeoutLayer::with_status_code(
					StatusCode::GATEWAY_TIMEOUT,
					settings.timeout,
				)),
		)
		.layer(middleware::from_fn(normalize_request_id))
}

async fn normalize_request_id(mut request: Request, next: Next) -> Response {
	// Accept only bounded UUIDs, never arbitrary caller content in logs.
	let valid = request
		.headers()
		.get("x-request-id")
		.and_then(|value| value.to_str().ok())
		.filter(|value| value.len() == 36)
		.and_then(|value| uuid::Uuid::parse_str(value).ok());
	if valid.is_none() {
		request.headers_mut().remove("x-request-id");
	}
	next.run(request).await
}

#[derive(Clone)]
pub(crate) struct SseSlots(pub Arc<tokio::sync::Semaphore>);
struct SseLease {
	_permit: tokio::sync::OwnedSemaphorePermit,
}
impl Drop for SseLease {
	fn drop(&mut self) {
		metrics::gauge!("aidash_sse_connections").decrement(1);
		metrics::counter!("aidash_sse_disconnects_total").increment(1);
	}
}

/// Hold admission for the lifetime of the body, including an unpolled body.
pub(crate) async fn sse_admission(
	axum::extract::State(slots): axum::extract::State<SseSlots>,
	request: Request,
	next: Next,
) -> Response {
	let Ok(permit) = slots.0.clone().try_acquire_owned() else {
		return (
			StatusCode::SERVICE_UNAVAILABLE,
			[(header::RETRY_AFTER, "1")],
			"SSE connection limit reached",
		)
			.into_response();
	};
	let response = next.run(request).await;
	if !response.status().is_success() {
		return response;
	}
	metrics::gauge!("aidash_sse_connections").increment(1);
	let lease = SseLease { _permit: permit };
	let (parts, body) = response.into_parts();
	let stream = async_stream::stream! {
		let _lease = lease;
		use futures_util::StreamExt;
		let mut stream = body.into_data_stream();
		while let Some(chunk) = stream.next().await { yield chunk; }
	};
	Response::from_parts(parts, Body::from_stream(stream))
}

/// Application JSON boundary: validation details never echo submitted values.
pub(crate) struct ValidatedJson<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for ValidatedJson<T>
where
	S: Send + Sync,
	T: serde::de::DeserializeOwned + validator::Validate,
{
	type Rejection = Response;
	async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
		match axum_valid::Valid::<axum::Json<T>>::from_request(request, state).await {
			Ok(axum_valid::Valid(axum::Json(value))) => Ok(Self(value)),
			Err(axum_valid::ValidationRejection::Valid(errors)) => {
				let mut fields: Vec<_> =
					errors.errors().keys().map(|field| field.as_ref()).collect();
				fields.sort_unstable();
				Err(
					crate::Error::Invalid(format!("invalid request fields: {}", fields.join(", ")))
						.into_response(),
				)
			}
			Err(axum_valid::ValidationRejection::Inner(error)) => Err((
				error.status(),
				axum::Json(serde_json::json!({"error": "invalid JSON request"})),
			)
				.into_response()),
		}
	}
}

/// Apply the same privacy policy to successful and rejected private responses.
pub(crate) fn private_responses<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
	use tower_http::set_header::SetResponseHeaderLayer;
	router
		.layer(SetResponseHeaderLayer::overriding(
			header::CACHE_CONTROL,
			HeaderValue::from_static("no-store"),
		))
		.layer(SetResponseHeaderLayer::overriding(
			header::REFERRER_POLICY,
			HeaderValue::from_static("no-referrer"),
		))
}

pub(crate) fn nonblank(value: &str) -> Result<(), validator::ValidationError> {
	if value.trim().is_empty() {
		Err(validator::ValidationError::new("nonblank"))
	} else {
		Ok(())
	}
}

/// Drop-safe gauge for worker cancellation and early returns.
pub(crate) struct ActiveExecution;
impl ActiveExecution {
	pub(crate) fn begin() -> Self {
		metrics::gauge!("aidash_worker_active_steps").increment(1);
		Self
	}
}
impl Drop for ActiveExecution {
	fn drop(&mut self) {
		metrics::gauge!("aidash_worker_active_steps").decrement(1);
	}
}

#[cfg(test)]
mod tests;
