//! Scratch Space HTTP endpoints.
use super::super::{
	serializers::scratch::{ScratchCreate, ScratchList, ScratchMember},
	services::scratch::{Operation, execute},
};
use crate::{authorization::identity::Actor, federation::Federation, http::json::Json};
use aidash_domain::scratch::ScratchSettings;
use reinhardt::http::ViewResult;
use reinhardt::{Path, Query, Response, delete, get, patch, post};
use uuid::Uuid;

#[post("/api/scratch-spaces", name = "scratch-create", auth = "protected")]
pub async fn create(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Json(input): Json<ScratchCreate>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Create(input)).await)
}
#[get("/api/scratch-spaces", name = "scratch-list", auth = "protected")]
pub async fn list(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Query(input): Query<ScratchList>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::List(input.node)).await)
}
#[get(
	"/api/scratch-spaces/limits",
	name = "scratch-limits",
	auth = "protected"
)]
pub async fn limits(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Query(input): Query<ScratchList>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Limits(input.node)).await)
}
#[get("/api/scratch-spaces/{id}", name = "scratch-read", auth = "protected")]
pub async fn read(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Read(id)).await)
}
#[patch(
	"/api/scratch-spaces/{id}/settings",
	name = "scratch-settings",
	auth = "protected"
)]
pub async fn settings(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ScratchSettings>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Settings(id, input)).await)
}
#[post(
	"/api/scratch-spaces/{id}/members",
	name = "scratch-member-add",
	auth = "protected"
)]
pub async fn add_member(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ScratchMember>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::AddMember(id, input.subject)).await)
}
#[post(
	"/api/scratch-spaces/{id}/members/remove",
	name = "scratch-member-remove",
	auth = "protected"
)]
pub async fn remove_member(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ScratchMember>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::RemoveMember(id, input.subject)).await)
}
#[post(
	"/api/scratch-spaces/{id}/clear",
	name = "scratch-clear",
	auth = "protected"
)]
pub async fn clear(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Clear(id)).await)
}
#[delete(
	"/api/scratch-spaces/{id}",
	name = "scratch-delete",
	auth = "protected"
)]
pub async fn remove(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Delete(id)).await)
}
