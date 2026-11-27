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
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::management::ThreadInput;
use crate::apps::workspaces::serializers::management::TitleInput;
use crate::apps::workspaces::services::management::CollaborationManagement;

#[get("/api/tasks", name = "task-list", auth = "protected")]
pub async fn task_list(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Query(page): Query<PageQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.task_list(actor, page).await)
}

#[get("/api/threads", name = "thread-list", auth = "protected")]
pub async fn thread_list(
	#[inject] service: Depends<crate::apps::execution::services::management::HarnessManagement>,
	#[inject] actor: Actor,
	Query(query): Query<crate::apps::execution::serializers::state::ThreadPageQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.thread_page(actor, query).await)
}

#[post("/api/threads", name = "thread-create", auth = "protected")]
pub async fn thread_create(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	validated_json::Json(input): validated_json::Json<ThreadInput>,
) -> ViewResult<Response> {
	crate::http::json(service.thread_create(actor, input).await)
}

#[get("/api/threads/{id}", name = "thread-get", auth = "protected")]
pub async fn thread_get(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Query(query): Query<crate::apps::workspaces::serializers::management::ThreadSnapshotQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.thread_get(actor, id, query.events_before).await)
}

#[patch("/api/threads/{id}", name = "thread-update", auth = "protected")]
pub async fn thread_update(
	#[inject] service: Depends<CollaborationManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<TitleInput>,
) -> ViewResult<Response> {
	crate::http::json(service.thread_update(actor, id, input).await)
}

#[post("/api/threads/{id}/tasks", name = "task-create", auth = "protected")]
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
	"/api/threads/{id}/messages",
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
