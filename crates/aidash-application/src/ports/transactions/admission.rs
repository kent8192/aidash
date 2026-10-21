//! Admission borrows one native transaction, including inherited authority locks.
use crate::Result;
use aidash_domain::transactions::{
	Manifest,
	authority::{Origin, Status},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[async_trait]
pub trait AdmissionScope: Send {
	fn node_id(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	async fn status(&mut self, id: Uuid) -> Result<Status>;
	async fn match_origin(&mut self, id: Uuid, origin: Option<&Origin>) -> Result<()>;
	/// Whether the submitting Node's Tenant registered `node` for atomic
	/// transactions. A submission with no origin has no Tenant and so no Node.
	/// Whether the transaction's coordinator Node may hold reservations on
	/// `node`: its Tenant allows atomic transactions with that Node. A
	/// submission with no origin is the Deployment Administrator's, and acts as
	/// the coordinator Node's own Tenant.
	/// The Tenant that holds `node`, when `node` is a Node of this Deployment.
	async fn node_tenant(&mut self, node: &str) -> Result<Option<String>>;
	async fn trusted(
		&mut self,
		coordinator: &str,
		node: &str,
		origin: Option<&Origin>,
	) -> Result<bool>;
	/// Admission is serialized by transaction ID and checks the immutable manifest
	/// again under its lock, then inserts participant votes and history atomically.
	async fn admit(&mut self, manifest: &Manifest) -> Result<Status>;
	async fn bind_origin(&mut self, id: Uuid, origin: &Origin) -> Result<()>;
}

#[async_trait]
pub trait OwnedAdmissionScope: Send {
	/// The borrowed view retains this scope's existing physical transaction.
	fn admission(&mut self) -> Box<dyn AdmissionScope + '_>;
	async fn commit(self: Box<Self>) -> Result<()>;
}

#[async_trait]
pub trait AdmissionRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn OwnedAdmissionScope>>;
	async fn fault(&self, id: Uuid, point: &str) -> Result<()>;
	fn wake(&self);
}
