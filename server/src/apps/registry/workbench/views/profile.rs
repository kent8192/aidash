//! Native HTTP endpoints.
use crate::apps::registry::workbench::profile::TestProfiles;
use crate::apps::registry::workbench::serializers::profile::{ProfileInput, ProfileQuery};
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::Depends;
use reinhardt::Path;
use reinhardt::Query;
use reinhardt::Response;
use reinhardt::http::ViewResult;
use reinhardt::{get, put};

#[get(
	"/api/workbench/test-profiles",
	name = "workbench-test-profiles",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<TestProfiles>,
	#[inject] actor: Actor,
	Query(query): Query<ProfileQuery>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, query).await)
}

#[put(
	"/api/workbench/test-profiles/{tenant}/{id}",
	name = "workbench-put-test-profile",
	auth = "protected"
)]
pub async fn put(
	#[inject] service: Depends<TestProfiles>,
	#[inject] actor: Actor,
	Path((tenant, id)): Path<(String, String)>,
	Json(input): Json<ProfileInput>,
) -> ViewResult<Response> {
	crate::http::json(service.put(actor, (tenant, id), input).await)
}
