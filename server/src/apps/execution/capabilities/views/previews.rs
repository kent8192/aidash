//! Preview ticket and availability endpoints (ADR 0068).
use crate::{
	apps::execution::capabilities::serializers::previews::{
		LivePreviewStatus, LivePreviewTicketInput, PreviewAvailability, PreviewLimitInput,
		PreviewTicket, PreviewTicketInput, TenantPreviewLimits,
	},
	authorization::identity::Actor,
	capabilities::endpoints::{self as api, CapabilitiesManagement},
};
use reinhardt::{Depends, Json, Path, Request, Response, get, http::ViewResult, post, put};
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

#[get(
	"/api/runs/{id}/live-preview",
	name = "!core_live_preview",
	auth = "protected"
)]
pub async fn live_preview(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::live_preview(service.runtime.clone(), actor, id).await)
}

#[post(
	"/api/runs/{id}/live-preview/stop",
	name = "!core_live_preview_stop",
	auth = "protected"
)]
pub async fn live_preview_stop(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::live_preview_stop(service.runtime.clone(), actor, id).await)
}

#[post(
	"/api/previews/live-tickets",
	name = "!core_preview_live_ticket",
	auth = "protected"
)]
pub async fn live_ticket(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	request: Request,
	Json(input): Json<LivePreviewTicketInput>,
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
		api::preview_live_ticket(service.runtime.clone(), actor, input, origin)
			.await
			.map(PreviewTicket::from),
	)
}

#[get(
	"/api/preview-limits/{tenant}",
	name = "!core_preview_limits",
	auth = "protected"
)]
pub async fn preview_limits(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(api::preview_limits(service.runtime.clone(), actor, tenant).await)
}

#[put(
	"/api/preview-limits/{tenant}",
	name = "!core_preview_limits_update",
	auth = "protected"
)]
pub async fn set_preview_limits(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
	Json(input): Json<PreviewLimitInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	if input.tenant != tenant {
		return crate::http::json(Err::<TenantPreviewLimits, _>(crate::Error::Invalid(
			"tenant mismatch".into(),
		)));
	}
	crate::http::json(api::set_preview_limits(service.runtime.clone(), actor, input).await)
}
