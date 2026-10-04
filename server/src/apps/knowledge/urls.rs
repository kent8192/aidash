//! Application endpoint registration and access policies.
use super::views::entries;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.endpoint(super::views::peer::query)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
		.endpoint(super::views::peer::verify)
		.with_route_middleware(AccessBoundary::peer().with_visibility())
		.endpoint(super::views::provenance::run_receipt)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::configure)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(entries::index)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::put)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::entries)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::delete)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::reindex)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::search)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::history)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(entries::cleanup)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
}
