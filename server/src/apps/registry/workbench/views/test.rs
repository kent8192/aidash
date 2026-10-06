//! Native HTTP endpoints.
use crate::apps::registry::workbench::serializers::test::{TestInput, TestLimits};
use crate::apps::registry::workbench::test::BehavioralTests;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post, put};
use uuid::Uuid;

#[get(
	"/api/workbench/drafts/{id}/test-limits",
	name = "workbench-get-test-limits",
	auth = "protected"
)]
pub async fn get_limits(
	#[inject] service: Depends<BehavioralTests>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.get_limits(actor, id).await)
}

#[put(
	"/api/workbench/test-limits/{tenant}",
	name = "workbench-set-test-limits",
	auth = "protected"
)]
pub async fn set_limits(
	#[inject] service: Depends<BehavioralTests>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
	Json(input): Json<TestLimits>,
) -> ViewResult<Response> {
	crate::http::json(service.set_limits(actor, tenant, input).await)
}

#[get(
	"/api/workbench/drafts/{id}/tests",
	name = "workbench-test-sessions",
	auth = "protected"
)]
pub async fn sessions(
	#[inject] service: Depends<BehavioralTests>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.sessions(actor, id).await)
}

#[post(
	"/api/workbench/tests/{id}/stop",
	name = "workbench-stop-test",
	auth = "protected"
)]
pub async fn stop(
	#[inject] service: Depends<BehavioralTests>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(service.stop(actor, id).await)
}

#[post(
	"/api/workbench/drafts/{id}/tests",
	name = "workbench-start-test",
	auth = "protected"
)]
pub async fn start(
	#[inject] service: Depends<BehavioralTests>,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(input): Json<TestInput>,
) -> ViewResult<Response> {
	crate::http::json(service.start(actor, id, input).await)
}
