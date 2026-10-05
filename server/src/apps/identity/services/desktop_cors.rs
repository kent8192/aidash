//! Preserve the bundled desktop transport policy with Reinhardt middleware.
use async_trait::async_trait;
use reinhardt::conf::CorsSettings;
use reinhardt::http::{Handler, Middleware, ViewResult};
use reinhardt::middleware::cors::create_cors_middleware_from_settings;
use reinhardt::{Request, Response, StatusCode};
use std::sync::Arc;

pub struct DesktopCors;

#[async_trait]
impl Middleware for DesktopCors {
	async fn process(&self, request: Request, next: Arc<dyn Handler>) -> ViewResult<Response> {
		let preflight = request.method == http::Method::OPTIONS;
		let mut settings = CorsSettings::default();
		settings.allow_origins = [
			"tauri://localhost",
			"http://tauri.localhost",
			"http://127.0.0.1:1420",
		]
		.map(str::to_owned)
		.to_vec();
		settings.allow_methods = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
			.map(str::to_owned)
			.to_vec();
		settings.allow_headers = [
			"authorization",
			"content-type",
			"x-aidash-context",
			"last-event-id",
			"idempotency-key",
		]
		.map(str::to_owned)
		.to_vec();
		settings.allow_credentials = false;
		let mut response = create_cors_middleware_from_settings(&settings)
			.process(request, next)
			.await?;
		// The original desktop client uses a bodyless 200 preflight and has no
		// preflight cache directive. Retain that transport contract.
		if preflight {
			response.status = StatusCode::OK;
			response
				.headers
				.remove(http::header::ACCESS_CONTROL_MAX_AGE);
		}
		if response
			.headers
			.contains_key(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
		{
			response.headers.insert(
				http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
				http::HeaderValue::from_static("x-aidash-event-cursor, x-aidash-next-offset"),
			);
		}
		Ok(response)
	}
}
