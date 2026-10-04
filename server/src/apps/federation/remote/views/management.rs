//! Management HTTP endpoints.
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::post;
use uuid::Uuid;

use crate::apps::federation::remote::serializers::management::DelegateInput;
use crate::apps::federation::remote::serializers::management::RemoteActionInput;
use crate::apps::federation::remote::services::management::RemoteManagement;

#[post("/api/tasks/{id}/delegate", name = "task-delegate", auth = "protected")]
pub async fn task_delegate(
	#[inject] service: Depends<RemoteManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<DelegateInput>,
) -> ViewResult<Response> {
	crate::http::json(service.task_delegate(actor, id, input).await)
}

#[post("/api/remote", name = "remote-action", auth = "protected")]
pub async fn remote_action(
	#[inject] service: Depends<RemoteManagement>,
	Json(input): Json<RemoteActionInput>,
) -> ViewResult<Response> {
	crate::http::json(service.remote_action(input).await)
}
