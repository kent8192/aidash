//! Aggregate each app without changing the external URL namespace.
use crate::apps::{
	execution, federation, identity, knowledge, marketplace, operations, registry, workspaces,
};
use reinhardt::{UnifiedRouter, routes};
use std::sync::Arc;

#[routes]
pub fn routes() -> UnifiedRouter {
	UnifiedRouter::new()
		.mount("/", identity::urls::server_url_patterns())
		.mount("/", execution::capabilities::urls::server_url_patterns())
		.mount("/", workspaces::urls::server_url_patterns())
		.mount("/", execution::generation::urls::server_url_patterns())
		.mount("/", operations::urls::server_url_patterns())
		.mount("/", federation::peer::urls::server_url_patterns())
		.mount("/", marketplace::urls::server_url_patterns())
		.mount("/", registry::urls::server_url_patterns())
		.mount("/", federation::remote::urls::server_url_patterns())
		.mount("/", knowledge::urls::server_url_patterns())
		.mount("/", federation::transactions::urls::server_url_patterns())
		.mount("/", registry::workbench::urls::server_url_patterns())
		.mount("/", execution::urls::server_url_patterns())
		.with_exception_handler(Arc::new(crate::http::ApiErrors))
		.with_middleware(crate::http::Gateway)
}

#[cfg(test)]
mod tests;
