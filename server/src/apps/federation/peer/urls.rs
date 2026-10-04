//! Application endpoint registration and access policies.
use super::views::management;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.endpoint(management::peer_create)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::discover)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::mesh)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::identity)
		.with_route_middleware(AccessBoundary::public().with_visibility())
		.endpoint(management::peer_discover)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
		.endpoint(management::peer_agent)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
		.endpoint(management::peer_offer)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
		.endpoint(management::peer_workspace)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
		.endpoint(management::peer_observe)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
		.endpoint(management::peer_control)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
}
