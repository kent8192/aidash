//! Compose only the owning distribution app.
use crate::{Marketplace, MarketplaceHandler};
use std::sync::Arc;
pub fn handler(service: &Arc<Marketplace>) -> MarketplaceHandler {
	crate::apps::distribution::urls::handler(service)
}
