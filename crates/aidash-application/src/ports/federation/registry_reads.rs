//! Current remote provenance borrows the reader's authority and enabled peer locks.
use crate::Result;
use aidash_domain::{
	federation::{dependencies::Reference as Dependency, registry_reads::Reference, route::Route},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
/// A remote read: its Node, the Agent's id and version, the digest and metadata read, and
/// whether the Node belongs to the reader's own Tenant.
pub type RegistryJournalRow = (String, String, String, String, Value, bool);
/// What a Run's reads of Agents found on other Nodes are to one reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteReads {
	Visible,
	/// `registry.read` is denied on an Agent found on a Node of the reader's own Tenant,
	/// which is the Tenant's own definition (#856).
	Definition(EntityRef),
	/// Another Node refused or could not confirm it, or its record no longer holds.
	Unconfirmed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
	Verified,
	Rejected,
	/// The peer did not answer: a timeout, `5xx`, a transport error, an
	/// unreadable answer or a pending transaction.
	Unavailable,
	/// Nobody was asked: a same-Deployment route with no in-process runtime.
	NotAsked,
}
#[async_trait]
pub trait RegistryTransport: Send + Sync {
	async fn verify(&self, route: &Route, references: &[Reference]) -> Verification;
}
#[async_trait]
pub trait RegistryReadScope: Send {
	fn identity(&self) -> (&str, &str);
	fn cached(&self, run: Uuid) -> Option<RemoteReads>;
	fn remember(&mut self, run: Uuid, reads: RemoteReads);
	fn frontier(&mut self) -> Option<&mut Vec<Dependency>>;
	/// Whether a Node already failed to answer within this authority (#751).
	fn unavailable(&self, node: &str) -> bool;
	/// As for the dependency walk: called each time a walk meets such a Node.
	fn mark_unavailable(&mut self, node: &str);
	/// The check in progress stops at a Node already known to be silent, without
	/// asking it again, so the row it belongs to is unconfirmed (#770).
	fn skipped(&mut self, _node: &str) {}
	/// `node` answered, with a verdict or a refusal, so it is no longer remembered
	/// as silent.
	fn answered(&mut self, _node: &str) {}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	fn catalog_resource(&self, entry: &Entry) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<RegistryJournalRow>>;
	async fn route(&mut self, node: &str) -> Result<Option<Route>>;
}
#[async_trait]
pub trait RegistryVerificationScope: Send {
	/// The owned receiving Node, named for policy and carrying its current attributes.
	async fn node_resource(&mut self) -> Result<Resource>;
	fn catalog_resource(&self, entry: &Entry) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	/// The definition under `registry.read` alone: a Catalog Approval does not decide who may read.
	async fn entry(&mut self, reference: &EntityRef) -> Result<Entry>;
}
