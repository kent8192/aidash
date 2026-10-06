//! Application endpoint registration and access policies.
use super::views::requests;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::UnifiedRouter;

pub fn url_patterns() -> UnifiedRouter {
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(super::views::peer::cancel_generation)
			.with_route_middleware(AccessBoundary::peer().with_visibility())
			.endpoint(super::views::peer::describe_generation)
			.with_route_middleware(AccessBoundary::peer().with_visibility())
			.endpoint(super::views::peer::prepare_generation)
			.with_route_middleware(AccessBoundary::peer().with_visibility())
			.endpoint(super::views::peer::reserve_usage)
			.with_route_middleware(AccessBoundary::peer().with_visibility())
			.endpoint(super::views::peer::verify_usage)
			.with_route_middleware(AccessBoundary::peer().with_visibility())
			.endpoint(super::views::peer::finalize_usage)
			.with_route_middleware(AccessBoundary::peer().with_visibility())
			.endpoint(super::views::foreign::request)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(super::views::foreign::cancel)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::set_policy)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::policies)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::assign)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::requests)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::control)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::history)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::usage)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(requests::spec)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
	})
}
