//! Management HTTP endpoints.
use crate::http::json::Json;
use crate::http::validated_json;
use crate::{
	apps::workspaces::serializers::tasks::PageQuery, authorization::identity::Actor, domain::*,
};
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, patch, post};
use uuid::Uuid;

use crate::apps::workspaces::serializers::management::AbandonInput;
use crate::apps::workspaces::serializers::management::ConversationInput;
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::management::StateInput;
use crate::apps::workspaces::serializers::management::WorkspaceInput;
use crate::apps::workspaces::services::management::CollaborationManagement;

#[get("/api/tasks", name = "task-list", auth = "protected")]
pub async fn task_list(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Query(page): Query<PageQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.task_list(actor, page).await)
}

#[post("/api/workspaces", name = "workspace-create", auth = "protected")]
pub async fn workspace_create(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	validated_json::Json(input): validated_json::Json<WorkspaceInput>,
) -> ViewResult<Response> {
	crate::http::json(service.workspace_create(actor, input).await)
}

#[get("/api/workspaces/{id}", name = "workspace-get", auth = "protected")]
pub async fn workspace_get(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.workspace_get(actor, id).await)
}

#[patch("/api/workspaces/{id}", name = "workspace-update", auth = "protected")]
pub async fn workspace_update(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<StateInput>,
) -> ViewResult<Response> {
	crate::http::json(service.workspace_update(actor, id, input).await)
}

#[post("/api/workspaces/{id}/tasks", name = "task-create", auth = "protected")]
pub async fn task_create(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	request: Request,
	Json(input): Json<NewTask>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.task_create(actor, id, request.headers.clone(), input)
			.await,
	)
}

#[post(
	"/api/workspaces/{id}/messages",
	name = "message-create",
	auth = "protected"
)]
pub async fn message_create(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<MessageInput>,
) -> ViewResult<Response> {
	crate::http::json(service.message_create(actor, id, input).await)
}

#[post("/api/tasks/{id}/abandon", name = "task-abandon", auth = "protected")]
pub async fn task_abandon(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<AbandonInput>,
) -> ViewResult<Response> {
	crate::http::json(service.task_abandon(actor, id, input).await)
}

#[post("/api/conversations", name = "conversation-create", auth = "protected")]
pub async fn conversation_create(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	validated_json::Json(input): validated_json::Json<ConversationInput>,
) -> ViewResult<Response> {
	crate::http::json(service.conversation_create(actor, input).await)
}
