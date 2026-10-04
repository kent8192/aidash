//! HTTP endpoints backed by native dependency injection.
use crate::apps::knowledge::services::entries::SemanticEntries;
use crate::apps::knowledge::*;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{delete, get, post};
use uuid::Uuid;

#[post(
	"/api/workspaces/{workspace}/semantic/index",
	name = "semantic-configure",
	auth = "protected"
)]
pub async fn configure(
	#[inject] service: Depends<SemanticEntries>,
	Path(workspace): Path<Uuid>,
	Json(input): Json<ConfigureIndex>,
) -> ViewResult<Response> {
	crate::http::json(service.configure(workspace, input).await)
}

#[get(
	"/api/workspaces/{workspace}/semantic/index",
	name = "semantic-index",
	auth = "protected"
)]
pub async fn index(
	#[inject] service: Depends<SemanticEntries>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.index(actor, workspace).await)
}

#[post(
	"/api/workspaces/{workspace}/semantic/entries",
	name = "semantic-put",
	auth = "protected"
)]
pub async fn put(
	#[inject] service: Depends<SemanticEntries>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	Json(input): Json<PutEntry>,
) -> ViewResult<Response> {
	crate::http::json(service.put(actor, workspace, input).await)
}

#[get(
	"/api/workspaces/{workspace}/semantic/entries",
	name = "semantic-entries",
	auth = "protected"
)]
pub async fn entries(
	#[inject] service: Depends<SemanticEntries>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.entries(actor, workspace).await)
}

#[delete(
	"/api/workspaces/{workspace}/semantic/entries/{id}",
	name = "semantic-delete",
	auth = "protected"
)]
pub async fn delete(
	#[inject] service: Depends<SemanticEntries>,
	#[inject] actor: Actor,
	Path((workspace, id)): Path<(Uuid, Uuid)>,
	Json(input): Json<Revision>,
) -> ViewResult<Response> {
	crate::http::json(service.delete(actor, (workspace, id), input).await)
}

#[post(
	"/api/workspaces/{workspace}/semantic/entries/{id}/reindex",
	name = "semantic-reindex",
	auth = "protected"
)]
pub async fn reindex(
	#[inject] service: Depends<SemanticEntries>,
	#[inject] actor: Actor,
	Path((workspace, id)): Path<(Uuid, Uuid)>,
	Json(input): Json<Revision>,
) -> ViewResult<Response> {
	crate::http::json(service.reindex(actor, (workspace, id), input).await)
}

#[post(
	"/api/workspaces/{workspace}/semantic/search",
	name = "semantic-search",
	auth = "protected"
)]
pub async fn search(
	#[inject] service: Depends<SemanticEntries>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	Json(input): Json<Search>,
) -> ViewResult<Response> {
	crate::http::json(service.search(actor, workspace, input).await)
}

#[get(
	"/api/workspaces/{workspace}/semantic/history",
	name = "semantic-history",
	auth = "protected"
)]
pub async fn history(
	#[inject] service: Depends<SemanticEntries>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.history(actor, workspace).await)
}

#[get(
	"/api/workspaces/{workspace}/semantic/cleanup",
	name = "semantic-cleanup",
	auth = "protected"
)]
pub async fn cleanup(
	#[inject] service: Depends<SemanticEntries>,
	Path(workspace): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.cleanup(workspace).await)
}
