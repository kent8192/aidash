//! An Approval Review reads one exact closure under the requester's authority.
use crate::Result;
use aidash_domain::{
	identity::catalog::CatalogRestriction,
	marketplace::Installation,
	nodes::NodeAddress,
	registry::{EntityRef, Entry, Projection},
};
use async_trait::async_trait;

/// The approval state of one exact version on the review Node.
pub struct ApprovalSnapshot {
	pub enabled: bool,
	pub revision: i64,
	pub restriction: Option<CatalogRestriction>,
}

/// The installed copy an exact version projects, with the activation fence the
/// review submission carries.
pub struct InstalledRevision {
	pub installation: Installation,
	pub revision: i64,
	pub digest: String,
	pub activation_revision: i64,
	pub active: bool,
}

/// Everything an Approval Review reads. The caller owns the transaction and
/// the Node selection; a review never writes.
#[async_trait]
pub trait ApprovalReviewScope: Send {
	fn tenant(&self) -> &str;
	fn node(&self) -> &NodeAddress;
	/// The raw exact version, approved or not.
	async fn definition(&mut self, reference: &EntityRef) -> Result<Option<Entry>>;
	async fn approval(&mut self, reference: &EntityRef) -> Result<Option<ApprovalSnapshot>>;
	async fn installed(&mut self, projection: &Projection) -> Result<Option<InstalledRevision>>;
	/// `registry.read` on the definition, decided without a refusal.
	async fn can_read(&mut self, entry: &Entry) -> Result<bool>;
	async fn require_read(&mut self, entry: &Entry) -> Result<()>;
	/// `catalog.approve` on the definition's approval resource.
	async fn can_approve(&mut self, entry: &Entry) -> Result<bool>;
	/// The Deployment offers the provider this definition needs.
	async fn provider_available(&mut self, entry: &Entry) -> Result<bool>;
	/// The Tenant Secret or connection this definition needs is present.
	async fn source_slot_available(&mut self, entry: &Entry) -> Result<bool>;
	/// Existing subject groups, or the default groups a new agent subject would receive.
	async fn subject_groups(
		&mut self,
		_subject: &str,
	) -> Result<std::collections::BTreeSet<String>>;
}
