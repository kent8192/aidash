//! Public process probes deliberately bypass authentication and visibility locks.
use super::super::services::lifecycle::ProcessHealth;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Response, StatusCode, get};

#[get("/live", name = "process-live")]
pub async fn live() -> ViewResult<Response> {
	Ok(Response::new(StatusCode::OK))
}

#[get("/ready", name = "process-ready")]
pub async fn ready(#[inject] health: Depends<ProcessHealth>) -> ViewResult<Response> {
	let status = if health.ready().await {
		StatusCode::OK
	} else {
		StatusCode::SERVICE_UNAVAILABLE
	};
	Ok(Response::new(status))
}
