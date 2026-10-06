//! Application endpoint registration and access policies.
use super::views::deployment;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::UnifiedRouter;

pub fn url_patterns() -> UnifiedRouter {
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(deployment::status)
			.with_route_middleware(AccessBoundary::operator())
	})
}
