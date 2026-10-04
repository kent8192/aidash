//! Retained local definitions use the inherited approval frontier and current catalog policy.
use crate::Result;
use aidash_domain::{
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
#[async_trait]
pub trait RetainedCatalogScope: Send {
	fn inherited(&self) -> bool;
	fn approved(&self, reference: &EntityRef) -> bool;
	fn remember(&mut self, reference: &EntityRef);
	async fn enabled(&mut self, reference: &EntityRef, lock: bool) -> Result<Option<bool>>;
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry>;
	fn resource(&self, entry: &Entry) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
}
