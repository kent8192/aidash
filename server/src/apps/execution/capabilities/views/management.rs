//! HTTP endpoints backed by injected application services.
use crate::capabilities::endpoints as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Path, Query, Request, Response, get, post};
use uuid::Uuid;

#[post(
	"/api/agents/{id}/capabilities",
	name = "!core_agent_configure",
	auth = "protected"
)]
pub async fn configure_agent(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<String>,
	Json(input): Json<crate::capabilities::configuration::Configure>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::configure_agent(service.runtime.clone(), actor, id, input).await)
}

#[get("/api/working-areas", name = "!working_area_list", auth = "protected")]
pub async fn areas(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Query(page): Query<AreaQuery>,
) -> ViewResult<Response> {
	crate::http::json(api::areas(service.runtime.clone(), actor, Query(page)).await)
}

#[get(
	"/api/runs/{id}/working-area",
	name = "!run_working_area",
	auth = "protected"
)]
pub async fn area_for_run(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::area_for_run(service.runtime.clone(), actor, id).await)
}

#[post(
	"/api/runs/{id}/files/read",
	name = "!core_file_read",
	auth = "protected"
)]
pub async fn file_read(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<FileRead>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::file_read(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/files/search",
	name = "!core_file_search",
	auth = "protected"
)]
pub async fn file_search(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<FileSearch>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::file_search(service.runtime.clone(), actor, id, input).await)
}

#[post("/api/runs/{id}/shell", name = "!core_shell", auth = "protected")]
pub async fn shell(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<Shell>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::shell(service.runtime.clone(), actor, id, input).await)
}

#[post("/api/runs/{id}/patch", name = "!core_apply_patch", auth = "protected")]
pub async fn apply_patch(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<Patch>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::apply_patch(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/shell/poll",
	name = "!core_shell_poll",
	auth = "protected"
)]
pub async fn shell_poll(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::shell_poll(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/shell/cancel",
	name = "!core_shell_cancel",
	auth = "protected"
)]
pub async fn shell_cancel(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::shell_cancel(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/files/materialize",
	name = "!core_file_materialize",
	auth = "protected"
)]
pub async fn materialize(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<Materialize>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::materialize(service.runtime.clone(), actor, id, input).await)
}

#[get(
	"/api/working-areas/{id}/session",
	name = "!core_session_status",
	auth = "protected"
)]
pub async fn session(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::session(service.runtime.clone(), actor, id).await)
}

#[post(
	"/api/working-areas/{id}/queue",
	name = "!core_session_enqueue",
	auth = "protected"
)]
pub async fn enqueue(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<Enqueue>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::enqueue(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/working-areas/{id}/steer",
	name = "!core_session_steer",
	auth = "protected"
)]
pub async fn steer(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<Steer>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::steer(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/capabilities/skills/import",
	name = "!core_skill_import",
	auth = "protected"
)]
pub async fn skill_import(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Json(input): Json<crate::skill_import::ImportRequest>,
) -> ViewResult<Response> {
	crate::http::json(api::skill_import(service.runtime.clone(), actor, input).await)
}

#[post(
	"/api/runs/{id}/skills/list",
	name = "!core_skill_list",
	auth = "protected"
)]
pub async fn skill_list(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::skills::SkillList>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::skill_list(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/skills/load",
	name = "!core_skill_load",
	auth = "protected"
)]
pub async fn skill_load(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::skills::SkillLoad>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::skill_load(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/skills/read",
	name = "!core_skill_read",
	auth = "protected"
)]
pub async fn skill_read(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::skills::SkillRead>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::skill_read(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/files/share",
	name = "!core_file_share",
	auth = "protected"
)]
pub async fn file_share(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::sharing::Share>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::file_share(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/workspaces/{workspace}/threads/{thread}/agents/{agent}/runs",
	name = "!core_thread_run",
	auth = "protected"
)]
pub async fn thread_run(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	request: Request,
	Json(input): Json<Enqueue>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	let workspace = request
		.path_params
		.get("workspace")
		.and_then(|value| value.parse::<Uuid>().ok())
		.ok_or_else(|| FrameworkError::Validation("invalid workspace path".into()))?;
	let thread = request
		.path_params
		.get("thread")
		.and_then(|value| value.parse::<Uuid>().ok())
		.ok_or_else(|| FrameworkError::Validation("invalid thread path".into()))?;
	let agent = request
		.path_params
		.get("agent")
		.map(str::to_owned)
		.ok_or_else(|| FrameworkError::Validation("missing agent path".into()))?;

	crate::http::json(
		api::thread_run(
			service.runtime.clone(),
			actor,
			(workspace, thread, agent),
			input,
		)
		.await,
	)
}

#[post(
	"/api/runs/{id}/outbound",
	name = "!core_outbound_get",
	auth = "protected"
)]
pub async fn outbound_get(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::approvals::Outbound>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::outbound_get(service.runtime.clone(), actor, id, input).await)
}

#[get(
	"/api/runs/{id}/outbound",
	name = "!core_outbound_history",
	auth = "protected"
)]
pub async fn outbound_history(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Query(page): Query<Page>,
) -> ViewResult<Response> {
	crate::http::json(api::outbound_history(service.runtime.clone(), actor, id, Query(page)).await)
}

#[get(
	"/api/capabilities/approvals",
	name = "!core_approval_list",
	auth = "protected"
)]
pub async fn approval_list(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Query(page): Query<Page>,
) -> ViewResult<Response> {
	crate::http::json(api::approval_list(service.runtime.clone(), actor, Query(page)).await)
}

#[post(
	"/api/capabilities/approvals/{id}/decide",
	name = "!core_approval_decide",
	auth = "protected"
)]
pub async fn approval_decide(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::approvals::ApprovalDecision>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::approval_decide(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/capabilities/approvals/{id}/revoke",
	name = "!core_approval_revoke",
	auth = "protected"
)]
pub async fn approval_revoke(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::approvals::Revoke>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::approval_revoke(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/capabilities/grants/{id}/revoke",
	name = "!core_grant_revoke",
	auth = "protected"
)]
pub async fn grant_revoke(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::approvals::Revoke>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::grant_revoke(service.runtime.clone(), actor, id, input).await)
}

#[post("/api/runs/{id}/python", name = "!core_python", auth = "protected")]
pub async fn python(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::python::Python>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::python(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/python/install",
	name = "!core_python_install",
	auth = "protected"
)]
pub async fn python_install(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::packages::Install>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::python_install(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/python/poll",
	name = "!core_python_poll",
	auth = "protected"
)]
pub async fn python_poll(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::python_poll(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/runs/{id}/python/cancel",
	name = "!core_python_cancel",
	auth = "protected"
)]
pub async fn python_cancel(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<OperationInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::python_cancel(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/references/uploads",
	name = "!reference_upload",
	auth = "protected"
)]
pub async fn reference_upload(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Json(input): Json<crate::capabilities::references::Upload>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::reference_upload(service.runtime.clone(), actor, input).await)
}

#[post(
	"/api/references/{id}/chunks",
	name = "!reference_chunk",
	auth = "protected"
)]
pub async fn reference_chunk(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::references::Chunk>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::reference_chunk(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/references/{id}/commit",
	name = "!reference_commit",
	auth = "protected"
)]
pub async fn reference_commit(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::reference_commit(service.runtime.clone(), actor, id).await)
}

#[get(
	"/api/references/{id}",
	name = "!reference_inspect",
	auth = "protected"
)]
pub async fn reference_inspect(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::reference_inspect(service.runtime.clone(), actor, id).await)
}

#[post(
	"/api/references/{id}/revoke",
	name = "!reference_revoke",
	auth = "protected"
)]
pub async fn reference_revoke(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ExpectedRevision>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::reference_revoke(service.runtime.clone(), actor, id, input).await)
}

#[get(
	"/api/references/{id}/download",
	name = "!reference_download",
	auth = "protected"
)]
pub async fn reference_download(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Query(input): Query<Offset>,
) -> ViewResult<Response> {
	crate::http::json(
		api::reference_download(service.runtime.clone(), actor, id, Query(input)).await,
	)
}

#[get(
	"/api/working-areas/{id}/files/{file}/download",
	name = "!core_file_download",
	auth = "protected"
)]
pub async fn file_download(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path((id, file)): Path<(Uuid, Uuid)>,
	Query(input): Query<Offset>,
) -> ViewResult<Response> {
	crate::http::json(
		api::file_download(service.runtime.clone(), actor, (id, file), Query(input)).await,
	)
}

#[get(
	"/api/working-files",
	name = "!core_file_management",
	auth = "protected"
)]
pub async fn managed_areas(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Query(input): Query<Page>,
) -> ViewResult<Response> {
	crate::http::json(api::managed_areas(service.runtime.clone(), actor, Query(input)).await)
}

#[post(
	"/api/working-areas/{id}/cleanup",
	name = "!core_cleanup",
	auth = "protected"
)]
pub async fn cleanup(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::cleanup::Cleanup>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::cleanup(service.runtime.clone(), actor, id, input).await)
}

#[get(
	"/api/file-cleanups/{id}",
	name = "!core_cleanup_status",
	auth = "protected"
)]
pub async fn cleanup_status(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::cleanup_status(service.runtime.clone(), actor, id).await)
}

#[post(
	"/api/file-cleanups/{id}/reconcile",
	name = "!core_cleanup_reconcile",
	auth = "protected"
)]
pub async fn cleanup_reconcile(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::cleanup_reconcile(service.runtime.clone(), actor, id).await)
}

#[post(
	"/api/working-areas/{id}/deletion-confirmation",
	name = "!core_deletion_confirmation",
	auth = "protected"
)]
pub async fn deletion_confirmation(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ExpectedRevision>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::deletion_confirmation(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/working-areas/{id}/restore",
	name = "!core_restore",
	auth = "protected"
)]
pub async fn restore(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<crate::capabilities::cleanup::Restore>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::restore(service.runtime.clone(), actor, id, input).await)
}

#[post(
	"/api/workspaces/{workspace}/working-areas/{id}/restore/new-thread",
	name = "!core_restore_new_thread",
	auth = "protected"
)]
pub async fn restore_new_thread(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path((workspace, id)): Path<(Uuid, Uuid)>,
	Json(input): Json<crate::capabilities::cleanup::RestoreNewThread>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(
		api::restore_new_thread(service.runtime.clone(), actor, (workspace, id), input).await,
	)
}

#[get(
	"/api/file-transfers/{id}",
	name = "!file_transfer_status",
	auth = "protected"
)]
pub async fn transfer_status(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::transfer_status(service.runtime.clone(), actor, id).await)
}

#[get(
	"/api/working-areas/{id}/transfers",
	name = "!file_transfer_history",
	auth = "protected"
)]
pub async fn transfer_history(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Query(page): Query<Page>,
) -> ViewResult<Response> {
	crate::http::json(api::transfer_history(service.runtime.clone(), actor, id, Query(page)).await)
}

#[post(
	"/api/file-transfers/{id}/reconcile",
	name = "!file_transfer_reconcile",
	auth = "protected"
)]
pub async fn transfer_reconcile(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::transfer_reconcile(service.runtime.clone(), actor, id).await)
}

#[get(
	"/api/file-recipients",
	name = "!file_recipient_list",
	auth = "protected"
)]
pub async fn transfer_recipients(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Query(input): Query<RecipientQuery>,
) -> ViewResult<Response> {
	crate::http::json(api::transfer_recipients(service.runtime.clone(), actor, Query(input)).await)
}

#[get(
	"/api/runs/{id}/core-operations",
	name = "!core_operation_history",
	auth = "protected"
)]
pub async fn operation_history(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Query(page): Query<Page>,
) -> ViewResult<Response> {
	crate::http::json(api::operation_history(service.runtime.clone(), actor, id, Query(page)).await)
}

#[get("/api/references", name = "!reference_list", auth = "protected")]
pub async fn reference_list(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Query(page): Query<Page>,
) -> ViewResult<Response> {
	crate::http::json(api::reference_list(service.runtime.clone(), actor, Query(page)).await)
}

#[post(
	"/api/workspaces/{workspace}/threads/{thread}/delete",
	name = "!core_thread_delete",
	auth = "protected"
)]
pub async fn thread_delete(
	#[inject] service: Depends<CapabilitiesManagement>,
	#[inject] actor: Actor,
	Path((workspace, thread)): Path<(Uuid, Uuid)>,
	Json(input): Json<crate::capabilities::thread_lifecycle::DeleteThread>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(
		api::thread_delete(service.runtime.clone(), actor, (workspace, thread), input).await,
	)
}

use crate::authorization::identity::Actor;
use crate::capabilities::contracts::*;

use reinhardt::core::exception::Error as FrameworkError;
