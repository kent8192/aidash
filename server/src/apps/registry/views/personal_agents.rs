//! Native HTTP endpoints.
use crate::apps::registry::knowledge::PersonalAgents;
use crate::apps::registry::serializers::knowledge::PersonalAgent;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Request;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::post;

#[post(
	"/api/agents/personal",
	name = "personal-agent-create",
	auth = "protected"
)]
pub async fn create(
	#[inject] service: Depends<PersonalAgents>,
	#[inject] actor: crate::authorization::identity::Actor,
	request: Request,
	Json(input): Json<PersonalAgent>,
) -> ViewResult<Response> {
	crate::http::json(service.create(actor, request.headers, input).await)
}
