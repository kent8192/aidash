//! Preview ticket and availability endpoints (ADR 0068).
use crate::http::json::Json;
use crate::{
	apps::execution::capabilities::serializers::previews::PreviewAvailability,
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

/// V's dashboard asks its own Deployment for a ticket to a delegated Run on H.
#[post(
	"/api/executions/{id}/preview-tickets",
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

/// V exposes H's scoped Live preview metadata.
#[get(
	"/api/executions/{id}/remote-preview",
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
