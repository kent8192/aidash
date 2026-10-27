//! Preview ticket and availability endpoints (ADR 0068).
use crate::http::json::Json;
use crate::{
	apps::execution::capabilities::serializers::previews::{
		PreviewAvailability, PreviewTicket, PreviewTicketInput,
	},
	authorization::identity::Actor,
	capabilities::endpoints::{self as api, CapabilitiesManagement},
};
use aidash_domain::federation::preview::Target;
use reinhardt::{Depends, Path, Request, Response, get, http::ViewResult, post};
use uuid::Uuid;

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

/// V's dashboard asks its own Deployment for a ticket to a delegated Run on H.
#[post(
	"/api/runs/{id}/preview-tickets",
	name = "!core_remote_preview_ticket",
	auth = "protected"
)]
pub async fn remote_ticket(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	request: Request,
	Json(target): Json<Target>,
) -> ViewResult<Response> {
	let origin = request
		.headers
		.get("origin")
		.and_then(|value| value.to_str().ok());
	crate::http::json(
		crate::apps::execution::capabilities::services::federation_previews::request(
			service.runtime.clone(),
			actor,
			id,
			target,
			origin,
		)
		.await,
	)
}

/// V exposes H's scoped Run File version and Live preview metadata.
#[get(
	"/api/runs/{id}/remote-preview",
	name = "!core_remote_preview_inspect",
	auth = "protected"
)]
pub async fn remote_inspect(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(
		crate::apps::execution::capabilities::services::federation_previews::inspect_remote(
			service.runtime.clone(),
			actor,
			id,
		)
		.await,
	)
}
