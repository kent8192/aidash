//! Native HTTP endpoints.
use crate::apps::identity::identity::Actor;
use crate::apps::identity::remote::RemoteGrants;
use crate::apps::identity::serializers::remote::PrepareInput;
use crate::apps::identity::serializers::remote::VerifyInput;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::post;
use uuid::Uuid;

#[post(
	"/api/tasks/{id}/remote-grants",
	name = "remote-grant-prepare",
	auth = "protected"
)]
pub async fn prepare(
	#[inject] service: Depends<RemoteGrants>,
	#[inject] actor: Actor,
	Path(task_id): Path<Uuid>,
	Json(input): Json<PrepareInput>,
) -> ViewResult<Response> {
	crate::http::json(service.prepare(actor, task_id, input).await)
}

#[post(
	"/api/tasks/{id}/remote-grants/{grant}/revoke",
	name = "remote-grant-revoke",
	auth = "protected"
)]
pub async fn revoke(
	#[inject] service: Depends<RemoteGrants>,
	#[inject] actor: Actor,
	Path((task_id, id)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(service.revoke(actor, (task_id, id)).await)
}

#[post(
	"/federation/v0.1/scoped/execution/grants/describe",
	name = "remote-grants-describe",
	auth = "protected"
)]
pub async fn describe(
	#[inject] service: Depends<RemoteGrants>,
	request: Request,
	Json(input): Json<VerifyInput>,
) -> ViewResult<Response> {
	crate::http::json(service.describe(request.headers, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/grants/verify",
	name = "remote-grants-verify",
	auth = "protected"
)]
pub async fn verify(
	#[inject] service: Depends<RemoteGrants>,
	request: Request,
	Json(input): Json<VerifyInput>,
) -> ViewResult<Response> {
	crate::http::json(service.verify(request.headers, input).await)
}

#[post(
	"/federation/v0.1/scoped/execution/grants/snapshot",
	name = "remote-grants-snapshot",
	auth = "protected"
)]
pub async fn snapshot(
	#[inject] service: Depends<RemoteGrants>,
	request: Request,
	Json(input): Json<VerifyInput>,
) -> ViewResult<Response> {
	crate::http::json(service.snapshot(request.headers, input).await)
}

#[post(
	"/api/tasks/{id}/remote-grants/inspect",
	name = "remote-agent-inspection",
	auth = "protected"
)]
pub async fn agent_inspection(
	#[inject] service: Depends<RemoteGrants>,
	#[inject] actor: Actor,
	Path(task_id): Path<Uuid>,
	Json(input): Json<aidash_domain::federation::execution::AgentInspectionInput>,
) -> ViewResult<Response> {
	crate::http::json(service.agent_inspection(actor, task_id, input).await)
}
