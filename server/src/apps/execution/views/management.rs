//! Management HTTP endpoints.
use crate::http::json::Json;
use crate::{apps::workspaces::serializers::tasks::PageQuery, authorization::identity::Actor};
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};
use serde_json::Value;
use uuid::Uuid;

use crate::apps::execution::serializers::management::ClaimInput;
use crate::apps::execution::serializers::management::ControlInput;
use crate::apps::execution::serializers::management::EventQuery;
use crate::apps::execution::serializers::management::InferenceStreamQuery;
use crate::apps::execution::serializers::management::{HumanListQuery, RunListQuery};
use crate::apps::execution::services::management::HarnessManagement;
use crate::apps::workspaces::serializers::management::MessageInput;

/// Reads one page of candidates. Filtered rows can leave a short page with a cursor.
/// The opaque cursor currently contains the last scanned Run ID, restricted to
/// a Task and Workspace the subject may read; individual Runs may be hidden.
/// Run visibility retains its dependent checks, including all Human Requests.
#[get("/api/tasks/{id}/runs", name = "task-runs", auth = "protected")]
pub async fn task_runs(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Query(q): Query<RunListQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.task_runs(actor, id, q).await)
}

/// Reads one page of Human Request candidates; filtered rows can leave a cursor on a short page.
/// The opaque cursor currently contains the last scanned request ID, restricted
/// to a Workspace the subject may read; individual requests may be hidden.
/// Each Run's visibility is checked once per request; its dependent checks can
/// still read and decide all Human Requests of that Run.
#[get("/api/human-requests", name = "human-requests", auth = "protected")]
pub async fn human_requests(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Query(q): Query<HumanListQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.human_requests(actor, q).await)
}

#[get("/api/state", name = "state", auth = "protected")]
pub async fn state(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	crate::http::response(service.state(actor).await)
}

#[get(
	"/api/providers/openrouter/models",
	name = "openrouter-models",
	auth = "protected"
)]
pub async fn openrouter_models(
	#[inject] service: Depends<HarnessManagement>,
) -> ViewResult<Response> {
	crate::http::json(service.openrouter_models().await)
}

#[post("/api/tasks/{id}/claim", name = "task-claim", auth = "protected")]
pub async fn task_claim(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ClaimInput>,
) -> ViewResult<Response> {
	crate::http::json(service.task_claim(actor, id, input).await)
}

#[get("/api/runs/{id}", name = "run-get", auth = "protected")]
pub async fn run_get(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Query(page): Query<PageQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.run_get(actor, id, page).await)
}

/// One Source citation of the Run. A reader who may not read the Run, and a
/// reference the Run does not hold, both get the same 404.
#[get(
	"/api/runs/{id}/evidence/{reference}",
	name = "run-evidence",
	auth = "protected"
)]
pub async fn run_evidence(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path((id, reference)): Path<(Uuid, String)>,
) -> ViewResult<Response> {
	crate::http::json(service.run_evidence(actor, id, &reference).await)
}

#[post("/api/runs/{id}/control", name = "run-control", auth = "protected")]
pub async fn run_control(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<ControlInput>,
) -> ViewResult<Response> {
	crate::http::json(service.run_control(actor, id, input).await)
}

#[post("/api/runs/{id}/message", name = "run-message", auth = "protected")]
pub async fn run_message(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<MessageInput>,
) -> ViewResult<Response> {
	crate::http::json(service.run_message(actor, id, input).await)
}

#[post(
	"/api/human-requests/{id}/answer",
	name = "human-answer",
	auth = "protected"
)]
pub async fn human_answer(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(response): Json<Value>,
) -> ViewResult<Response> {
	crate::http::json(service.human_answer(actor, id, response).await)
}

#[get("/api/events", name = "events", auth = "protected")]
pub async fn events(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Query(q): Query<EventQuery>,
) -> ViewResult<Response> {
	crate::http::response(service.events(actor, q).await)
}

#[get("/api/events/stream", name = "stream", auth = "protected")]
pub async fn stream(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	request: Request,
	Query(q): Query<EventQuery>,
) -> ViewResult<Response> {
	match service
		.stream(
			actor,
			browser,
			request.headers.clone(),
			q,
			request.extensions.get::<crate::http::SseLeaseHandle>(),
		)
		.await
	{
		Ok(stream) => Ok(Response::ok()
			.with_stream(stream)
			.with_header("Content-Type", "text/event-stream")
			.with_header("Cache-Control", "no-cache")),
		Err(error) => Ok(error.http_response()),
	}
}

#[get(
	"/api/runs/{id}/inference/stream",
	name = "run-inference-stream",
	auth = "protected"
)]
pub async fn run_inference_stream(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	request: Request,
	Path(id): Path<Uuid>,
	Query(q): Query<InferenceStreamQuery>,
) -> ViewResult<Response> {
	match service
		.run_inference_stream(
			actor,
			browser,
			request.headers.clone(),
			id,
			q,
			request.extensions.get::<crate::http::SseLeaseHandle>(),
		)
		.await
	{
		Ok(stream) => Ok(Response::ok()
			.with_stream(stream)
			.with_header("Content-Type", "text/event-stream")
			.with_header("Cache-Control", "no-cache")),
		Err(error) => Ok(error.http_response()),
	}
}

#[get("/health", name = "health", auth = "public")]
pub async fn health(#[inject] service: Depends<HarnessManagement>) -> ViewResult<Response> {
	crate::http::json(service.health().await)
}

/// The Run's research usage against its limits and its estimated cost, for
/// anyone who may read the Run.
#[get("/api/runs/{id}/research", name = "run-research", auth = "protected")]
pub async fn run_research(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.run_research(actor, id).await)
}

/// The Research providers of this Deployment, whether each is set up and
/// approved for the reader's Tenant, and the per-Run research limits.
#[get(
	"/api/research/providers",
	name = "research-providers",
	auth = "protected"
)]
pub async fn research_providers(
	#[inject] service: Depends<HarnessManagement>,
	#[inject] actor: Actor,
	Query(input): Query<crate::apps::execution::serializers::runs::ResearchProvidersQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.research_providers(actor, input).await)
}
