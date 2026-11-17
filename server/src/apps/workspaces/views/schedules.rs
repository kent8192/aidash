//! Schedule HTTP endpoints (#557).
use super::super::{
	serializers::schedules::{ListScope, ScheduleListQuery, SuccessorInput},
	services::schedules::{Operation, execute},
};
use crate::{authorization::identity::Actor, federation::Federation, http::json::Json};
use aidash_application::schedules::Listing;
use aidash_domain::schedules::{Definition, Limits, Patch};
use reinhardt::http::ViewResult;
use reinhardt::{Path, Query, Response, delete, get, patch, post, put};
use uuid::Uuid;

#[get(
	"/api/workspaces/{workspace}/schedules",
	name = "workspace-schedules",
	auth = "protected"
)]
pub async fn workspace_list(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(
		execute(
			&runtime,
			actor,
			Operation::List(Listing::Workspace(workspace)),
		)
		.await,
	)
}
#[post(
	"/api/workspaces/{workspace}/schedules",
	name = "schedule-create",
	auth = "protected"
)]
pub async fn create(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	Json(input): Json<Definition>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Create(workspace, input)).await)
}
#[get("/api/schedules", name = "schedule-list", auth = "protected")]
pub async fn list(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Query(input): Query<ScheduleListQuery>,
) -> ViewResult<Response> {
	let listing = match input.scope {
		ListScope::Mine => Listing::Mine,
		ListScope::Tenant => Listing::Tenant,
	};
	crate::http::json(execute(&runtime, actor, Operation::List(listing)).await)
}
#[get(
	"/api/schedules/notices",
	name = "schedule-notices",
	auth = "protected"
)]
pub async fn notices(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Notices).await)
}
#[get("/api/schedules/limits", name = "schedule-limits", auth = "protected")]
pub async fn limits(#[inject] runtime: Federation, #[inject] actor: Actor) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Limits).await)
}
#[put(
	"/api/schedules/limits",
	name = "schedule-limits-update",
	auth = "protected"
)]
pub async fn set_limits(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Json(input): Json<Limits>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::SetLimits(input)).await)
}
#[get("/api/schedules/{id}", name = "schedule-read", auth = "protected")]
pub async fn read(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Read(id)).await)
}
#[patch("/api/schedules/{id}", name = "schedule-update", auth = "protected")]
pub async fn update(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<Patch>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Update(id, input)).await)
}
#[post(
	"/api/schedules/{id}/pause",
	name = "schedule-pause",
	auth = "protected"
)]
pub async fn pause(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Pause(id)).await)
}
#[post(
	"/api/schedules/{id}/resume",
	name = "schedule-resume",
	auth = "protected"
)]
pub async fn resume(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Resume(id)).await)
}
#[post(
	"/api/schedules/{id}/successor",
	name = "schedule-successor",
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
#[post(
	"/api/schedules/{id}/run-now",
	name = "schedule-run-now",
	auth = "protected"
)]
pub async fn run_now(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::RunNow(id)).await)
}
#[delete("/api/schedules/{id}", name = "schedule-delete", auth = "protected")]
pub async fn remove(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Delete(id)).await)
}
