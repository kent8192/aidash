//! Aggregate each app without changing the external URL namespace.
use crate::apps::{
	execution, federation, identity, knowledge, marketplace, operations, registry, workspaces,
};
use reinhardt::{UnifiedRouter, routes};
use std::sync::Arc;

#[routes]
pub fn routes() -> UnifiedRouter {
	UnifiedRouter::new()
		.mount_unified("/", identity::urls::url_patterns())
		.mount_unified("/", execution::capabilities::urls::url_patterns())
		.mount_unified("/", workspaces::urls::url_patterns())
		.mount_unified("/", execution::generation::urls::url_patterns())
		.mount_unified("/", operations::urls::url_patterns())
		.mount_unified("/", federation::peer::urls::url_patterns())
		.mount_unified("/", marketplace::urls::url_patterns())
		.mount_unified("/", registry::urls::url_patterns())
		.mount_unified("/", federation::remote::urls::url_patterns())
		.mount_unified("/", knowledge::urls::url_patterns())
		.mount_unified("/", federation::transactions::urls::url_patterns())
		.mount_unified("/", registry::workbench::urls::url_patterns())
		.mount_unified("/", execution::urls::url_patterns())
		.with_exception_handler(Arc::new(crate::http::ApiErrors))
		.with_middleware(identity::services::desktop_cors::DesktopCors)
		.with_middleware(crate::http::Gateway)
}

#[cfg(test)]
mod tests;
