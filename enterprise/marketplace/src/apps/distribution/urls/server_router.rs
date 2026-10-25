//! Explicit native registration of the app's typed server functions.
use super::super::server_fn::*;
use reinhardt::{ServerRouter, pages::server_fn::ServerFnRouterExt};
pub fn server_url_patterns() -> ServerRouter {
	ServerRouter::new()
		.server_fn(marketplace_session::marker)
		.server_fn(marketplace_catalog::marker)
		.server_fn(marketplace_detail::marker)
		.server_fn(marketplace_publishers::marker)
		.server_fn(marketplace_publisher_plugins::marker)
		.server_fn(marketplace_library::marker)
		.server_fn(marketplace_create_publisher::marker)
		.server_fn(marketplace_create_plugin::marker)
		.server_fn(marketplace_publish::marker)
		.server_fn(marketplace_acquire::marker)
		.server_fn(marketplace_logout::marker)
		.server_fn(marketplace_connections::marker)
}
