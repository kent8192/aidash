//! Current remote provenance borrows the reader's authority and enabled peer locks.
use crate::Result;
use aidash_domain::{
	federation::{Peer, dependencies::Reference as Dependency, registry_reads::Reference},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub type RegistryJournalRow = (String, String, String, String, Value);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
	Verified,
	Rejected,
	Unavailable,
}
#[async_trait]
pub trait RegistryTransport: Send + Sync {
	async fn verify(
		&self,
		peer: &Peer,
		tenant: &str,
		subject: &str,
		references: &[Reference],
	) -> Verification;
}
#[async_trait]
pub trait RegistryReadScope: Send {
	fn protocol(&self) -> &str;
	fn identity(&self) -> (&str, &str);
	fn cached(&self, run: Uuid) -> Option<bool>;
	fn remember(&mut self, run: Uuid, visible: bool);
	fn frontier(&mut self) -> Option<&mut Vec<Dependency>>;
	fn unavailable(&self, node: &str) -> bool;
	fn mark_unavailable(&mut self, node: &str);
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	fn catalog_resource(&self, entry: &Entry) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<RegistryJournalRow>>;
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>>;
}
#[async_trait]
pub trait RegistryVerificationScope: Send {
	fn node(&self) -> &str;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	fn catalog_resource(&self, entry: &Entry) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn entry(&mut self, reference: &EntityRef) -> Result<Entry>;
}
