//! Application endpoint registration and access policies.
use super::views::management;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.endpoint(management::task_delegate)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::remote_action)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
}
