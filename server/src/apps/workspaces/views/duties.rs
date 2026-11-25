//! Duty HTTP endpoints (#1011, from the Schedules of #557).
use super::super::{
	serializers::duties::{DutyListQuery, ListScope, SuccessorInput},
	services::duties::{Operation, execute},
};
use crate::{authorization::identity::Actor, federation::Federation, http::json::Json};
use aidash_application::duties::Listing;
use aidash_domain::duties::{Definition, Patch};
use reinhardt::http::ViewResult;
use reinhardt::{Path, Query, Response, delete, get, patch, post};
use uuid::Uuid;

#[get(
	"/api/threads/{thread}/duties",
	name = "thread-duties",
	auth = "protected"
)]
pub async fn thread_list(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::List(Listing::Thread(thread))).await)
}
#[post(
	"/api/threads/{thread}/duties",
	name = "duty-create",
	auth = "protected"
)]
pub async fn create(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
	Json(input): Json<Definition>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Create(thread, input)).await)
}
#[get("/api/duties", name = "duty-list", auth = "protected")]
pub async fn list(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Query(input): Query<DutyListQuery>,
) -> ViewResult<Response> {
	let listing = match input.scope {
		ListScope::Mine => Listing::Mine,
		ListScope::Tenant => Listing::Tenant,
	};
	crate::http::json(execute(&runtime, actor, Operation::List(listing)).await)
}
#[get("/api/duties/notices", name = "duty-notices", auth = "protected")]
pub async fn notices(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Notices).await)
}
#[get("/api/duties/{id}", name = "duty-read", auth = "protected")]
pub async fn read(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Read(id)).await)
}
#[patch("/api/duties/{id}", name = "duty-update", auth = "protected")]
pub async fn update(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<Patch>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Update(id, input)).await)
}
#[post("/api/duties/{id}/pause", name = "duty-pause", auth = "protected")]
pub async fn pause(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Pause(id)).await)
}
#[post("/api/duties/{id}/resume", name = "duty-resume", auth = "protected")]
pub async fn resume(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Resume(id)).await)
}
#[post(
	"/api/duties/{id}/successor",
	name = "duty-successor",
	auth = "protected"
)]
pub async fn successor(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<SuccessorInput>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Successor(id, input.subject)).await)
}
/// The named successor takes over a Duty whose Requester left; it stays
/// stopped until they resume it (#991, ADR 0145).
#[post(
	"/api/duties/{id}/take-over",
	name = "duty-take-over",
	auth = "protected"
)]
pub async fn take_over(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::TakeOver(id)).await)
}
#[post("/api/duties/{id}/run-now", name = "duty-run-now", auth = "protected")]
pub async fn run_now(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::RunNow(id)).await)
}
#[delete("/api/duties/{id}", name = "duty-delete", auth = "protected")]
pub async fn remove(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Delete(id)).await)
}
