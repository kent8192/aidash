//! Application endpoint registration and access policies.
use super::views::management;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::UnifiedRouter;

pub fn url_patterns() -> UnifiedRouter {
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(management::discover)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::identity)
			.with_route_middleware(AccessBoundary::public().with_visibility())
	})
}
