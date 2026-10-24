//! HTTP endpoints backed by native dependency injection.
use crate::apps::execution::generation::serializers::requests::{AssignInput, PolicyUpdate};
use crate::apps::execution::generation::services::requests::GenerationRequests;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, post};
use uuid::Uuid;

#[post(
	"/api/generation/{tenant}/policies/{id}",
	name = "generation-set-policy",
	auth = "protected"
)]
pub async fn set_policy(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, String)>,
	Json(input): Json<PolicyUpdate>,
) -> ViewResult<Response> {
	crate::http::json(service.set_policy(actor, (tenant, id), input).await)
}

#[get(
	"/api/generation/{tenant}/policies",
	name = "generation-policies",
	auth = "protected"
)]
pub async fn policies(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.policies(actor, tenant).await)
}

#[post(
	"/api/generation/{tenant}/tasks/{id}/assign",
	name = "generation-assign",
	auth = "protected"
)]
pub async fn assign(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
	Json(input): Json<AssignInput>,
) -> ViewResult<Response> {
	crate::http::json(service.assign(actor, (tenant, id), input).await)
}

#[get(
	"/api/generation/{tenant}/requests",
	name = "generation-requests",
	auth = "protected"
)]
pub async fn requests(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path(tenant): Path<String>,
) -> ViewResult<Response> {
	crate::http::json(service.requests(actor, tenant).await)
}

#[post(
	"/api/generation/{tenant}/requests/{id}/control",
	name = "generation-control",
	auth = "protected"
)]
pub async fn control(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
	Json(input): Json<crate::apps::execution::generation::lifecycle::Control>,
) -> ViewResult<Response> {
	crate::http::json(service.control(actor, (tenant, id), input).await)
}

#[get(
	"/api/generation/{tenant}/requests/{id}/history",
	name = "generation-history",
	auth = "protected"
)]
pub async fn history(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(service.history(actor, (tenant, id)).await)
}

#[get(
	"/api/generation/{tenant}/requests/{id}/usage",
	name = "generation-usage",
	auth = "protected"
)]
pub async fn usage(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(service.usage(actor, (tenant, id)).await)
}

#[get(
	"/api/generation/{tenant}/requests/{id}/spec",
	name = "generation-spec",
	auth = "protected"
)]
pub async fn spec(
	#[inject] service: Depends<GenerationRequests>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(service.spec(actor, (tenant, id)).await)
}
