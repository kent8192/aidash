//! Application endpoint registration and access policies.
use super::views::deployment;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.endpoint(deployment::status)
		.with_route_middleware(AccessBoundary::operator())
}
