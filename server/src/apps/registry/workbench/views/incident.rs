//! Native HTTP endpoints.
use crate::apps::registry::workbench::incident::Incidents;
use crate::apps::registry::workbench::serializers::incident::{CreateIncident, UpdateIncident};
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post, put};
use uuid::Uuid;

#[post(
	"/api/workbench/versions/{id}/{version}/incidents",
	name = "workbench-create-incident",
	auth = "protected"
)]
pub async fn create(
	#[inject] service: Depends<Incidents>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
	Json(input): Json<CreateIncident>,
) -> ViewResult<Response> {
	crate::http::json(service.create(actor, (id, version), input).await)
}

#[get(
	"/api/workbench/versions/{id}/{version}/incidents",
	name = "workbench-list-incidents",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<Incidents>,
	#[inject] actor: Actor,
	Path((id, version)): Path<(String, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, (id, version)).await)
}

#[get(
	"/api/workbench/incidents/{id}",
	name = "workbench-get-incident",
	auth = "protected"
)]
pub async fn get(
	#[inject] service: Depends<Incidents>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.get(actor, id).await)
}

#[put(
	"/api/workbench/incidents/{id}",
	name = "workbench-update-incident",
	auth = "protected"
)]
pub async fn update(
	#[inject] service: Depends<Incidents>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<UpdateIncident>,
) -> ViewResult<Response> {
	crate::http::json(service.update(actor, id, input).await)
}

#[get(
	"/api/workbench/incidents/{id}/events",
	name = "workbench-incident-events",
	auth = "protected"
)]
pub async fn events(
	#[inject] service: Depends<Incidents>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.events(actor, id).await)
}
