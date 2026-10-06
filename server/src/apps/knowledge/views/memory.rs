//! Unit-level memory routes. There is no whole-bank replacement endpoint.
use crate::{
	apps::knowledge::services::native_memory::{
		CreateParticipant, NativeMemory, Operation, ReadBank, UpgradeParticipant,
	},
	authorization::identity::Actor,
	http::json::Json,
};
use aidash_domain::memory::Mutation;
use reinhardt::{Depends, Path, Response, http::ViewResult, post};
use uuid::Uuid;

#[post(
	"/api/workspaces/{workspace}/memory/operate",
	name = "memory-operate",
	auth = "protected"
)]
pub async fn operate(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	Json(input): Json<Operation>,
) -> ViewResult<Response> {
	crate::http::json(service.operate(actor, workspace, input).await)
}
#[post(
	"/api/workspaces/{workspace}/memory/participants/{participant}/upgrade",
	name = "memory-participant-upgrade",
	auth = "protected"
)]
pub async fn upgrade(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path((workspace, participant)): Path<(Uuid, Uuid)>,
	Json(input): Json<UpgradeParticipant>,
) -> ViewResult<Response> {
	crate::http::json(service.upgrade(actor, workspace, participant, input).await)
}

#[post(
	"/api/workspaces/{workspace}/memory/units/query",
	name = "memory-units",
	auth = "protected"
)]
pub async fn units(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	Json(input): Json<ReadBank>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, workspace, input).await)
}
#[post(
	"/api/workspaces/{workspace}/memory/units/mutate",
	name = "memory-mutate",
	auth = "protected"
)]
pub async fn mutate(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	Json(input): Json<Mutation>,
) -> ViewResult<Response> {
	crate::http::json(service.mutate(actor, workspace, input).await)
}
#[post(
	"/api/workspaces/{workspace}/memory/participants",
	name = "memory-participant",
	auth = "protected"
)]
pub async fn participant(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	Json(input): Json<CreateParticipant>,
) -> ViewResult<Response> {
	crate::http::json(service.participant(actor, workspace, input).await)
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ParticipantCursor {
	pub after: Option<Uuid>,
}
#[reinhardt::get(
	"/api/workspaces/{workspace}/memory/participants",
	name = "memory-participants",
	auth = "protected"
)]
pub async fn participants(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(workspace): Path<Uuid>,
	reinhardt::Query(input): reinhardt::Query<ParticipantCursor>,
) -> ViewResult<Response> {
	crate::http::json(service.participants(actor, workspace, input.after).await)
}
#[post(
	"/api/workspaces/{workspace}/tasks/{task}/memory-participant",
	name = "memory-participant-assignment",
	auth = "protected"
)]
pub async fn assign(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path((workspace, task)): Path<(Uuid, Uuid)>,
	Json(input): Json<crate::semantic::native_memory::AssignmentChange>,
) -> ViewResult<Response> {
	crate::http::json(service.assign(actor, workspace, task, input).await)
}

#[reinhardt::get(
	"/api/workspaces/{workspace}/tasks/{task}/memory-participant",
	name = "memory-participant-current",
	auth = "protected"
)]
pub async fn assignment(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path((workspace, task)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(service.assignment(actor, workspace, task).await)
}
