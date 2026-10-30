//! Current catalog authority, inherited approval fences and exact retained definitions.
use crate::Result;
use aidash_domain::{
	identity::catalog::RestrictionChange,
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
#[async_trait]
pub trait CatalogScope: Send {
	fn tenant(&self) -> &str;
	fn inherited_lease(&self) -> bool;
	fn approved(&self, reference: &EntityRef) -> bool;
	fn remember(&mut self, reference: &EntityRef);
	/// The subject that uses `reference` directly. An agent's exact version is
	/// used by whoever starts or delegates its Run; a definition an agent
	/// depends on is used by that agent.
	fn direct_user(&self, _reference: &EntityRef) -> String {
		String::new()
	}
	/// Whether the stored Catalog Approval restriction admits `direct_user`.
	/// Scopes without stored restrictions admit everyone, like an unrestricted
	/// approval.
	async fn permits_restriction(
		&mut self,
		_reference: &EntityRef,
		_direct_user: &str,
	) -> Result<bool> {
		Ok(true)
	}
	/// An exact admission this scope already proved under the same authority while
	/// its locks still hold. Scopes without such a record re-prove every admission.
	fn admitted(&self, _reference: &EntityRef, _action: &str) -> Option<Entry> {
		None
	}
	fn admit(&mut self, _reference: &EntityRef, _action: &str, _entry: &Entry) {}
	async fn distribution_lock(&mut self) -> Result<()>;
	async fn document(&mut self, reference: &EntityRef) -> Result<Option<Value>>;
	async fn documents(&mut self) -> Result<Vec<Value>>;
	fn resource(&self, entry: &Entry) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	/// Current Marketplace activation and dependency authority borrow this lease.
	async fn active(&mut self, entry: &Entry) -> Result<bool>;
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()>;
}

/// A protected mutation borrows the caller's transaction and authority locks.
/// Neither an approval update nor its history may commit independently.
#[async_trait]
pub trait CatalogMutation: Send {
	async fn registered(&mut self, reference: &EntityRef) -> Result<bool>;
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		node: &aidash_domain::nodes::NodeName,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
		restriction: RestrictionChange,
	) -> Result<Option<aidash_domain::identity::catalog::Binding>>;
	async fn history(
		&mut self,
		binding: &aidash_domain::identity::catalog::Binding,
		actor: &str,
		provider_warning_acknowledgment: Option<&str>,
	) -> Result<()>;
}

/// A Tenant subject's Catalog Approvals and the agent subjects they create
/// share the subject's transaction, its policy lock and its decision audit.
#[async_trait]
pub trait CatalogApprovalScope: CatalogMutation {
	fn tenant(&self) -> &str;
	/// The Node the approvals apply to: an approval is made per Node.
	fn node(&self) -> &aidash_domain::nodes::NodeAddress;
	/// The history label of the approving subject.
	fn actor(&self) -> &str;
	/// Any version in the Tenant's Registry, approved or not.
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry>;
	/// Whether the Tenant currently approves this exact version.
	async fn approved(&mut self, reference: &EntityRef) -> Result<bool>;
	/// Decide `action` on the definition's approval resource,
	/// `registry` `{kind}/{id}@{version}`.
	async fn require(&mut self, entry: &Entry, action: &str) -> Result<()>;
	/// Decide `action` on a subject of the Tenant's policy.
	async fn require_subject(
		&mut self,
		subject: &str,
		action: &str,
		groups: &std::collections::BTreeSet<String>,
	) -> Result<()>;
	fn policy(&mut self) -> &mut aidash_domain::policy::PolicyBundle;
	/// Persist the changed policy as its next revision.
	async fn save_policy(&mut self) -> Result<()>;
}

#[async_trait]
pub trait CatalogAdministrator: CatalogMutation {
	fn principal(&self) -> &aidash_domain::identity::Principal;
	async fn load_tenant(&mut self, tenant: &str) -> Result<()>;
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry>;
}

pub mod retained;
