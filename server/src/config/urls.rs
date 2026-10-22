//! Aggregate each app without changing the external URL namespace. Each group
//! of routes declares the Offerings that serve it; an Offering mounts only
//! its own, so a route it does not serve does not exist there.
use crate::apps::{
	execution, federation, identity, knowledge, marketplace, operations, registry, workspaces,
};
use aidash_domain::offering::Offering;
use reinhardt::{UnifiedRouter, routes};
use std::sync::Arc;

/// Which Offerings serve a group of routes.
pub type Offerings = &'static [Offering];
/// The default: every Offering.
pub const EVERY_OFFERING: Offerings = &[Offering::SelfHosted, Offering::Cloud, Offering::Dedicated];
/// Routes that are meaningless where Aidash runs the Deployment.
pub const SELF_HOSTED_ONLY: Offerings = &[Offering::SelfHosted];
/// Routes of the shared multi-Tenant Deployment, such as billing (#242).
pub const CLOUD_ONLY: Offerings = &[Offering::Cloud];

/// A group of routes and the Offerings that serve it.
type Group = (Offerings, fn() -> UnifiedRouter);

/// Community's route groups. Reinhardt cannot drop a mounted endpoint, so a
/// route that some Offering does not serve moves into a group of its own here.
const GROUPS: &[Group] = &[
	(EVERY_OFFERING, identity::urls::url_patterns),
	(EVERY_OFFERING, execution::capabilities::urls::url_patterns),
	(EVERY_OFFERING, workspaces::urls::url_patterns),
	(EVERY_OFFERING, execution::generation::urls::url_patterns),
	(EVERY_OFFERING, operations::urls::url_patterns),
	(EVERY_OFFERING, federation::peer::urls::url_patterns),
	(EVERY_OFFERING, marketplace::urls::url_patterns),
	(EVERY_OFFERING, registry::urls::url_patterns),
	(EVERY_OFFERING, federation::remote::urls::url_patterns),
	(EVERY_OFFERING, knowledge::urls::url_patterns),
	(EVERY_OFFERING, federation::transactions::urls::url_patterns),
	(EVERY_OFFERING, registry::workbench::urls::url_patterns),
	(EVERY_OFFERING, execution::urls::url_patterns),
];

/// The routes this process serves for its Deployment's Offering.
#[routes]
pub fn routes() -> UnifiedRouter {
	routes_for(crate::bootstrap::build::offering())
}

/// The routes `offering` serves: Community's groups that serve it and the
/// installed build's own.
pub fn routes_for(offering: Offering) -> UnifiedRouter {
	let router = match crate::bootstrap::build::current()
		.extension
		.routes(offering)
	{
		Some(extension) => UnifiedRouter::new().mount_unified("/", extension),
		None => UnifiedRouter::new(),
	};
	GROUPS
		.iter()
		.filter(|(offerings, _)| offerings.contains(&offering))
		.fold(router, |router, (_, group)| {
			router.mount_unified("/", group())
		})
		.with_exception_handler(Arc::new(crate::http::ApiErrors))
		.with_middleware(identity::services::desktop_cors::DesktopCors)
		.with_middleware(crate::http::Gateway)
}

#[cfg(test)]
mod tests;
