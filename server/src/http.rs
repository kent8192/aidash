//! HTTP response rendering shared by application views.
pub mod json;
pub mod validated_json;
use reinhardt::StatusCode;
use reinhardt::core::validators::Validate as ValidateRules;
use reinhardt::{Response, http::ViewResult};
use serde::Serialize;

pub fn json<T: Serialize>(result: crate::Result<T>) -> ViewResult<Response> {
	match result {
		Ok(value) => Response::ok().with_json(&value),
		Err(error) => Ok(error.http_response()),
	}
}

pub fn response(result: crate::Result<Response>) -> ViewResult<Response> {
	Ok(result.unwrap_or_else(crate::Error::http_response))
}

pub fn validate<T: ValidateRules>(value: &T) -> crate::Result<()> {
	value.validate().map_err(|error| {
		crate::Error::Invalid(format!(
			"invalid request fields: {}",
			error
				.field_errors()
				.keys()
				.map(|field| field.as_ref())
				.collect::<Vec<_>>()
				.join(", ")
		))
	})
}

pub fn json_status<T: Serialize>(
	result: crate::Result<T>,
	status: StatusCode,
) -> ViewResult<Response> {
	match result {
		Ok(value) => Response::new(status).with_json(&value),
		Err(error) => Ok(error.http_response()),
	}
}

pub fn status(result: crate::Result<StatusCode>) -> ViewResult<Response> {
	response(result.map(Response::new))
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

#[derive(Clone)]
pub(crate) struct SseSlots(pub Arc<tokio::sync::Semaphore>);
struct SseLeaseState {
	permit: Option<tokio::sync::OwnedSemaphorePermit>,
	counted: bool,
}
#[derive(Clone)]
pub(crate) struct SseLeaseHandle(Arc<std::sync::Mutex<SseLeaseState>>);
impl SseLeaseHandle {
	fn activate(&self) {
		let mut state = self.0.lock().expect("SSE admission");
		if state.permit.is_some() && !state.counted {
			state.counted = true;
			metrics::gauge!("aidash_sse_connections").increment(1);
		}
	}
	pub(crate) fn release(&self) {
		let mut state = self.0.lock().expect("SSE admission");
		state.permit.take();
		if state.counted {
			state.counted = false;
			metrics::gauge!("aidash_sse_connections").decrement(1);
			metrics::counter!("aidash_sse_disconnects_total").increment(1);
		}
	}
}
struct SseLease(SseLeaseHandle);
impl Drop for SseLease {
	fn drop(&mut self) {
		self.0.release();
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

use async_trait::async_trait;
use http::{HeaderName, HeaderValue, header};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::http::{ExceptionHandler, Handler, Middleware};
use reinhardt::{InjectionContext, Request, injectable};
use std::{
	collections::HashMap,
	net::IpAddr,
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub const BODY_LIMIT: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedPeer(pub String);
#[derive(Clone)]
pub struct Protection {
	settings: Settings,
	permits: Arc<Semaphore>,
	sse: SseSlots,
	buckets: Arc<Mutex<HashMap<String, (f64, Instant)>>>,
}
impl Protection {
	pub fn new(settings: Settings) -> Self {
		Self {
			permits: Arc::new(Semaphore::new(settings.concurrency)),
			sse: SseSlots(Arc::new(Semaphore::new(settings.sse_connections))),
			settings,
			buckets: Arc::new(Mutex::new(HashMap::new())),
		}
	}
	fn admit(&self, key: String, burst: u32, period: Duration) -> bool {
		let now = Instant::now();
		let mut buckets = self.buckets.lock().expect("HTTP admission");
		if buckets.len() >= 1024 {
			buckets.retain(|_, (_, updated)| {
				now.duration_since(*updated) < period.saturating_mul(burst)
			});
		}
		let entry = buckets.entry(key).or_insert((f64::from(burst), now));
		entry.0 = (entry.0 + now.duration_since(entry.1).as_secs_f64() / period.as_secs_f64())
			.min(f64::from(burst));
		entry.1 = now;
		if entry.0 < 1.0 {
			return false;
		}
		entry.0 -= 1.0;
		true
	}
	pub(crate) fn authenticated(
		&self,
		request: &Request,
		actor: Option<&crate::authorization::identity::Actor>,
	) -> bool {
		if let Some(peer) = request.extensions.get::<AuthenticatedPeer>() {
			return self.admit(
				format!("peer:{}", peer.0),
				self.settings.peer_burst,
				self.settings.peer_period,
			);
		}
		let key = match actor {
			Some(crate::authorization::identity::Actor::Operator) => "operator".into(),
			Some(crate::authorization::identity::Actor::Subject(subject)) => {
				format!("subject:{}:{}", subject.tenant, subject.subject)
			}
			None => return true,
		};
		self.admit(key, self.settings.actor_burst, self.settings.actor_period)
	}
	fn auth_ip(&self, request: &Request) -> Option<IpAddr> {
		let peer = request.remote_addr?.ip();
		if self.settings.auth_trusted_proxy_ips.contains(&peer) {
			let mut values = request.headers.get_all("x-real-ip").iter();
			let ip = values
				.next()
				.and_then(|v| v.to_str().ok())
				.and_then(|v| v.parse().ok());
			if values.next().is_none() && ip.is_some() {
				return ip;
			}
		}
		Some(peer)
	}
}
#[injectable(scope = "singleton")]
pub async fn provide_protection() -> DiResult<Protection> {
	Settings::from_env()
		.map(Protection::new)
		.map_err(|e| reinhardt::di::DiError::ProviderError(e.to_string()))
}
fn rejection(status: StatusCode, text: &str) -> Response {
	Response::new(status)
		.with_body(text.as_bytes().to_vec())
		.with_header("Retry-After", "1")
}
#[derive(Clone, Copy)]
pub struct Gateway;

/// Apply the application's safe error contract to native extractor failures.
pub struct ApiErrors;

#[async_trait]
impl ExceptionHandler for ApiErrors {
	async fn handle_exception(&self, request: &Request, error: FrameworkError) -> Response {
		let rejection = if matches!(&error, FrameworkError::ParamValidation(context)
			if context.param_type == reinhardt::core::exception::ParamType::Json)
		{
			request.extensions.get::<json::Rejection>()
		} else {
			None
		};
		if let Some(rejection) = rejection {
			return rejection.response();
		}
		crate::Error::from(error).http_response()
	}
}

#[async_trait]
impl Middleware for Gateway {
	async fn process(&self, mut request: Request, next: Arc<dyn Handler>) -> ViewResult<Response> {
		let context = request
			.get_di_context::<Arc<InjectionContext>>()
			.ok_or_else(|| {
				reinhardt::core::exception::Error::Internal(
					"HTTP dependency context is unavailable".into(),
				)
			})?;
		let protection = reinhardt::Depends::<Protection>::resolve_from_registry(&context, true)
			.await
			.map_err(|e| reinhardt::core::exception::Error::Internal(e.to_string()))?;
		let id = request
			.headers
			.get("x-request-id")
			.and_then(|v| v.to_str().ok())
			.filter(|v| v.len() == 36)
			.and_then(|v| uuid::Uuid::parse_str(v).ok())
			.unwrap_or_else(uuid::Uuid::new_v4)
			.to_string();
		request.headers.insert(
			HeaderName::from_static("x-request-id"),
			HeaderValue::from_str(&id).expect("UUID header"),
		);
		for name in ["authorization", "cookie", "x-aidash-csrf"] {
			if let Some(value) = request.headers.get_mut(name) {
				value.set_sensitive(true);
			}
		}
		let path = request.uri.path().to_owned();
		let private = (path.starts_with("/api") && path != "/api/openapi.json")
			|| path.starts_with("/federation")
			|| path.starts_with("/auth/");
		let sse = path == "/api/events/stream";
		let limit = if path.contains("/chunks") || path.contains("/scoped/files/") {
			6 * 1024 * 1024
		} else {
			BODY_LIMIT
		};
		let start = Instant::now();
		let response = if request.body().len() > limit {
			Response::new(StatusCode::PAYLOAD_TOO_LARGE)
				.with_body(b"request body too large".to_vec())
		} else if path.starts_with("/auth/")
			&& protection.auth_ip(&request).is_some_and(|ip| {
				!protection.admit(
					format!("auth:{ip}"),
					protection.settings.auth_burst,
					protection.settings.auth_period,
				)
			}) {
			rejection(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded")
		} else {
			match protection.permits.clone().try_acquire_owned() {
				Err(_) => rejection(StatusCode::SERVICE_UNAVAILABLE, "server overloaded"),
				Ok(permit) => {
					let lease = if sse {
						match protection.sse.0.clone().try_acquire_owned() {
							Ok(permit) => {
								Some(SseLeaseHandle(Arc::new(Mutex::new(SseLeaseState {
									permit: Some(permit),
									counted: false,
								}))))
							}
							Err(_) => {
								return Ok(private_response(
									rejection(
										StatusCode::SERVICE_UNAVAILABLE,
										"SSE connection limit reached",
									),
									&id,
									private,
								));
							}
						}
					} else {
						None
					};
					if let Some(lease) = &lease {
						request.extensions.insert(lease.clone());
					}
					let mut response = match tokio::time::timeout(
						protection.settings.timeout,
						next.handle(request),
					)
					.await
					{
						Ok(Ok(response)) => response,
						Ok(Err(error)) => crate::Error::from(error).http_response(),
						Err(_) => Response::new(StatusCode::GATEWAY_TIMEOUT)
							.with_body(b"request timed out".to_vec()),
					};
					drop(permit);
					if let Some(handle) = lease {
						if response.status.is_success() {
							handle.activate();
						}
						let lease = SseLease(handle);
						if let Some(mut body) = response.take_stream_body() {
							use futures_util::StreamExt;
							let stream = async_stream::stream! {let _lease=lease;while let Some(chunk)=body.next().await {yield chunk;}};
							response.with_stream(stream)
						} else {
							response
						}
					} else {
						response
					}
				}
			}
		};
		// Route names are bounded labels; raw paths, query values and credentials never enter logs.
		metrics::counter!("aidash_http_requests_total", "status" => response.status.as_u16().to_string()).increment(1);
		metrics::histogram!("aidash_http_request_duration_seconds")
			.record(start.elapsed().as_secs_f64());
		tracing::info!(request_id=%id,status=response.status.as_u16(),duration_ms=start.elapsed().as_secs_f64()*1000.0,"HTTP response");
		Ok(private_response(response, &id, private))
	}
}
fn private_response(mut response: Response, id: &str, private: bool) -> Response {
	response.headers.insert(
		HeaderName::from_static("x-request-id"),
		HeaderValue::from_str(id).expect("UUID header"),
	);
	response.headers.insert(
		header::X_CONTENT_TYPE_OPTIONS,
		HeaderValue::from_static("nosniff"),
	);
	if private {
		response
			.headers
			.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
		response.headers.insert(
			header::REFERRER_POLICY,
			HeaderValue::from_static("no-referrer"),
		);
	}
	if let Some(value) = response.headers.get_mut(header::SET_COOKIE) {
		value.set_sensitive(true);
	}
	response
}

use reinhardt::di::DiResult;
