//! HTTP endpoints backed by injected application services.
use crate::authorization::remote::execution as api;
use crate::http::json::Json;
use api::*;
use reinhardt::http::ViewResult;
use reinhardt::{Depends, Path, Request, Response, get, post};
use uuid::Uuid;

#[post(
	"/api/tasks/{task}/remote-grants/{id}/messages",
	name = "!remote_execution_message",
	auth = "protected"
)]
pub async fn message(
	#[inject] service: Depends<RemoteExecutionManagement>,
	#[inject] actor: Actor,
	Path((task, id)): Path<(Uuid, Uuid)>,
	Json(input): Json<RemoteExecutionMessageInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::message(service.runtime.clone(), actor, (task, id), input).await)
}

#[post(
	"/api/tasks/{task}/remote-grants/{id}/control",
	name = "!remote_execution_control",
	auth = "protected"
)]
pub async fn control(
	#[inject] service: Depends<RemoteExecutionManagement>,
	#[inject] actor: Actor,
	Path((task, id)): Path<(Uuid, Uuid)>,
	Json(input): Json<RemoteExecutionControlInput>,
) -> ViewResult<Response> {
	if let Err(error) = crate::http::validate(&input) {
		return Ok(error.http_response());
	}
	crate::http::json(api::control(service.runtime.clone(), actor, (task, id), input).await)
}

#[get(
	"/api/tasks/{task}/remote-executions",
	name = "!remote_execution_list",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<RemoteExecutionManagement>,
	#[inject] actor: Actor,
	Path(task): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(api::list(service.runtime.clone(), actor, task).await)
}

#[post(
	"/api/tasks/{task}/remote-grants/{id}/activate",
	name = "!remote_execution_activate",
	auth = "protected"
)]
pub async fn activate(
	#[inject] service: Depends<RemoteExecutionManagement>,
	#[inject] actor: Actor,
	Path((task, id)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(api::activate(service.runtime.clone(), actor, (task, id)).await)
}

#[post(
	"/federation/v0.1/scoped/execution/grants/activation",
	name = "remoteexecutionmanagement-activation-binding",
	auth = "protected"
)]
pub async fn activation_binding(
	#[inject] service: Depends<RemoteExecutionManagement>,
	request: Request,
	Json(input): Json<VerifyInput>,
) -> ViewResult<Response> {
	crate::http::json(
		api::activation_binding(service.runtime.clone(), request.headers.clone(), input).await,
	)
}

use crate::authorization::identity::Actor;

use crate::apps::identity::serializers::remote::VerifyInput;

#[post(
	"/api/tasks/{task}/remote-grants/{grant}/human-requests/answer",
	name = "!remote_human_answer",
	auth = "protected"
)]
pub async fn answer_human(
	#[inject] service: Depends<RemoteExecutionManagement>,
	#[inject] actor: Actor,
	Path((task, grant)): Path<(Uuid, Uuid)>,
	Json(input): Json<crate::apps::identity::serializers::remote_execution::RemoteHumanAnswer>,
) -> ViewResult<Response> {
	crate::http::json(
		aidash_application::authorization::home::answer_human(
			&crate::bootstrap::home_execution_repository(&service.runtime, actor),
			task,
			grant,
			input.id,
			input.response,
		)
		.await
		.map_err(crate::Error::from),
	)
}
