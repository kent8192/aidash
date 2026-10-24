//! Prometheus endpoint registered only on the dedicated metrics listener.
use super::super::services::metrics::ProcessMetrics;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Response, get};
#[get("/metrics", name = "!process_metrics")]
pub async fn metrics(#[inject] service: Depends<ProcessMetrics>) -> ViewResult<Response> {
	Ok(Response::ok()
		.with_header("content-type", "text/plain; version=0.0.4; charset=utf-8")
		.with_body(service.0.render()))
}
