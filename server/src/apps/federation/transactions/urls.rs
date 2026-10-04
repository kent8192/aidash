//! Application endpoint registration and access policies.
use super::views::protocol;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.endpoint(protocol::restore_peer)
		.with_route_middleware(AccessBoundary::operator())
		.endpoint(protocol::preflight)
		.with_route_middleware(AccessBoundary::peer())
		.endpoint(protocol::read_access)
		.with_route_middleware(AccessBoundary::peer())
		.endpoint(protocol::authority_ticket)
		.with_route_middleware(AccessBoundary::peer())
		.endpoint(protocol::submit)
		.with_route_middleware(AccessBoundary::authenticated())
		.endpoint(protocol::list)
		.with_route_middleware(AccessBoundary::authenticated())
		.endpoint(protocol::details)
		.with_route_middleware(AccessBoundary::authenticated())
		.endpoint(protocol::abort)
		.with_route_middleware(AccessBoundary::authenticated())
		.endpoint(protocol::participants)
		.with_route_middleware(AccessBoundary::operator())
		.endpoint(protocol::trust_list)
		.with_route_middleware(AccessBoundary::operator())
		.endpoint(protocol::trust)
		.with_route_middleware(AccessBoundary::operator())
		.endpoint(protocol::reserve)
		.with_route_middleware(AccessBoundary::peer())
		.endpoint(protocol::prepare)
		.with_route_middleware(AccessBoundary::peer())
		.endpoint(protocol::finish)
		.with_route_middleware(AccessBoundary::peer())
		.endpoint(protocol::decision)
		.with_route_middleware(AccessBoundary::peer())
}
