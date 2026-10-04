//! Application endpoint registration and access policies.
use super::views::management;
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::ServerRouter;

pub fn server_url_patterns() -> ServerRouter {
	probe_url_patterns()
		.endpoint(super::views::schema::document)
		.endpoint(management::state)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::openrouter_models)
		.with_route_middleware(AccessBoundary::operator().with_visibility())
		.endpoint(management::task_claim)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::run_get)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::run_control)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::run_message)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::human_answer)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::events)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::stream)
		.with_route_middleware(AccessBoundary::authenticated().with_visibility())
		.endpoint(management::health)
		.with_route_middleware(AccessBoundary::public())
		.endpoint(super::views::frontend::index)
		.endpoint(super::views::frontend::index_head)
		.endpoint(super::views::frontend::asset)
		.endpoint(super::views::frontend::asset_head)
}

pub fn probe_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.endpoint(super::views::lifecycle::live)
		.endpoint(super::views::lifecycle::ready)
}
