//! Preview ticket and availability endpoints (ADR 0068).
use crate::{
	apps::execution::capabilities::serializers::previews::{
		PreviewAvailability, PreviewTicket, PreviewTicketInput,
	},
	authorization::identity::Actor,
	capabilities::endpoints::{self as api, CapabilitiesManagement},
};
use reinhardt::{Depends, Json, Request, Response, get, http::ViewResult, post};

#[get(
	"/api/previews/availability",
	name = "!core_preview_availability",
	auth = "protected"
)]
pub async fn availability(
	#[inject] service: Depends<CapabilitiesManagement>,
) -> ViewResult<Response> {
	crate::http::json(Ok::<_, crate::Error>(PreviewAvailability::from(
		api::preview_availability(&service.runtime),
	)))
}

#[post(
	"/api/previews/tickets",
	name = "!core_preview_ticket",
	auth = "protected"
)]
pub async fn ticket(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	request: Request,
	Json(input): Json<PreviewTicketInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	let origin = request
		.headers
		.get("origin")
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned)
		.unwrap_or_else(|| service.runtime.config.dashboard_origin.clone());
	crate::http::json(
		api::preview_ticket(service.runtime.clone(), actor, input, origin)
			.await
			.map(PreviewTicket::from),
	)
}
