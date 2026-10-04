//! Native HTTP endpoints.
use crate::apps::operations::DeploymentObservations;
use crate::authorization::identity::Actor;
use aidash_domain::identity::Principal;
use reinhardt::Depends;
use reinhardt::Response;
use reinhardt::get;
use reinhardt::http::ViewResult;

#[get("/api/deployment", name = "deployment-status", auth = "protected")]
pub async fn status(
	#[inject] service: Depends<DeploymentObservations>,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	let principal = match &actor {
		Actor::Operator => Principal::Operator,
		Actor::Subject(subject) => Principal::Subject {
			tenant: subject.tenant.clone(),
			subject: subject.subject.clone(),
		},
	};
	crate::http::json(service.status(&principal).await.map_err(Into::into))
}
