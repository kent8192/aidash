//! Distribution app HTTP entry point.
use super::{Marketplace, MarketplaceHandler};
use std::sync::Arc;
pub fn handler(service: &Arc<Marketplace>) -> MarketplaceHandler {
	service.handler()
}
