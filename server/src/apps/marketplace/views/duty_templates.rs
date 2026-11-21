//! Duty template HTTP endpoints (#1011 D4). Browsing, reading and sharing a
//! template are the Marketplace's own package endpoints.
use super::super::serializers::duty_templates::{
	InstallDutyTemplate, PublishDutyTemplate, UpdateDutyTemplate,
};
use super::super::services::duty_templates as service;
use crate::{authorization::identity::Actor, federation::Federation, http::json::Json};
use reinhardt::http::ViewResult;
use reinhardt::{Path, Response, get, post};
use uuid::Uuid;

#[get(
	"/api/duties/{id}/template",
	name = "duty-template-draft",
	auth = "protected"
)]
pub async fn draft(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service::draft(&runtime, actor, id).await)
}

#[post(
	"/api/duties/{id}/template",
	name = "duty-template-publish",
	auth = "protected"
)]
pub async fn publish(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<PublishDutyTemplate>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service::publish(&runtime, actor, id, input).await)
}

#[post(
	"/api/marketplace/packages/{key}/duty",
	name = "marketplace-install-duty",
	auth = "protected"
)]
pub async fn install(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(key): Path<String>,
	Json(input): Json<InstallDutyTemplate>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service::install(&runtime, actor, key, input).await)
}

#[post(
	"/api/duties/{id}/template-update",
	name = "duty-template-update",
	auth = "protected"
)]
pub async fn update(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<UpdateDutyTemplate>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::response(service::update(&runtime, actor, id, input).await)
}
