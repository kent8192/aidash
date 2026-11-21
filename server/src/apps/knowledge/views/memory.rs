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
	"/api/threads/{thread}/memory/operate",
	name = "memory-operate",
	auth = "protected"
)]
pub async fn operate(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
	Json(input): Json<Operation>,
) -> ViewResult<Response> {
	crate::http::json(service.operate(actor, thread, input).await)
}
#[post(
	"/api/threads/{thread}/memory/participants/{participant}/upgrade",
	name = "memory-participant-upgrade",
	auth = "protected"
)]
pub async fn upgrade(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path((thread, participant)): Path<(Uuid, Uuid)>,
	Json(input): Json<UpgradeParticipant>,
) -> ViewResult<Response> {
	crate::http::json(service.upgrade(actor, thread, participant, input).await)
}

#[post(
	"/api/threads/{thread}/memory/units/query",
	name = "memory-units",
	auth = "protected"
)]
pub async fn units(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
	Json(input): Json<ReadBank>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, thread, input).await)
}
#[post(
	"/api/threads/{thread}/memory/units/mutate",
	name = "memory-mutate",
	auth = "protected"
)]
pub async fn mutate(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
	Json(input): Json<Mutation>,
) -> ViewResult<Response> {
	crate::http::json(service.mutate(actor, thread, input).await)
}
/// Content-free preview and post-change view of the visible Runs and derived
/// Units that depend on the given Units.
#[post(
	"/api/threads/{thread}/memory/units/impact",
	name = "memory-impact",
	auth = "protected"
)]
pub async fn impact(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
	Json(input): Json<aidash_domain::memory::impact::ImpactQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.impact(actor, thread, input).await)
}
#[post(
	"/api/threads/{thread}/memory/participants",
	name = "memory-participant",
	auth = "protected"
)]
pub async fn participant(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
	Json(input): Json<CreateParticipant>,
) -> ViewResult<Response> {
	crate::http::json(service.participant(actor, thread, input).await)
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ParticipantCursor {
	pub after: Option<Uuid>,
}
#[reinhardt::get(
	"/api/threads/{thread}/memory/participants",
	name = "memory-participants",
	auth = "protected"
)]
pub async fn participants(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path(thread): Path<Uuid>,
	reinhardt::Query(input): reinhardt::Query<ParticipantCursor>,
) -> ViewResult<Response> {
	crate::http::json(service.participants(actor, thread, input.after).await)
}
#[post(
	"/api/threads/{thread}/tasks/{task}/memory-participant",
	name = "memory-participant-assignment",
	auth = "protected"
)]
pub async fn assign(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path((thread, task)): Path<(Uuid, Uuid)>,
	Json(input): Json<crate::semantic::native_memory::AssignmentChange>,
) -> ViewResult<Response> {
	crate::http::json(service.assign(actor, thread, task, input).await)
}

#[reinhardt::get(
	"/api/threads/{thread}/tasks/{task}/memory-participant",
	name = "memory-participant-current",
	auth = "protected"
)]
pub async fn assignment(
	#[inject] service: Depends<NativeMemory>,
	#[inject] actor: Actor,
	Path((thread, task)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(service.assignment(actor, thread, task).await)
}
