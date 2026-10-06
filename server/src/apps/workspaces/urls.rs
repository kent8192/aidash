//! Application endpoint registration and access policies.
use super::views::{channels, management};
use crate::apps::identity::services::boundary::AccessBoundary;
use reinhardt::UnifiedRouter;

pub fn url_patterns() -> UnifiedRouter {
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(management::task_list)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::workspace_create)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::workspace_get)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::workspace_update)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::task_create)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::message_create)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::task_abandon)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(management::conversation_create)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(channels::channel_thread_create)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(channels::channel_message_create)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(channels::channel_message_history)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(channels::channel_attachment_upload)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(channels::channel_attachment_download)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
	})
}
