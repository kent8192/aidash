//! Application endpoint registration and access policies.
use super::views::{management, personal_agents};
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.endpoint(personal_agents::create)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::registry_list)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::registry_get)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::registry_create)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::skill_import)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::marketplace)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::package_publish)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::package_install)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
}
