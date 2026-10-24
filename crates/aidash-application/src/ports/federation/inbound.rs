//! What accepting an inbound Federation request needs to know about the
//! Deployment and its Tenants (ADR 0064).
use crate::Result;
use aidash_domain::{
	identity::peer_mapping::Mapping,
	nodes::{
		DeploymentId, NodeAddress, NodeName,
		keys::TrustedDeploymentKeys,
		registration::{NodeRegistration, PinnedKey},
	},
	tenancy::TenantName,
};
use async_trait::async_trait;
use uuid::Uuid;

/// A Peer Graph Grant as `accept_grant_viewer` needs it: the viewer
/// Deployment's pinned key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerGrant {
	pub key: PinnedKey,
	pub revision: i64,
}

/// Receiving trust belongs to this exact authority record, never globally to
/// the sending Deployment. Node registrations and Graph grants are independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivingKeyOwner {
	Registration {
		tenant: TenantName,
		node: NodeName,
		source_node: NodeAddress,
	},
	ViewerGrant {
		tenant: TenantName,
		viewer: DeploymentId,
		identity: Uuid,
	},
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivingKeys {
	pub keys: TrustedDeploymentKeys,
	pub keys_revision: i64,
}

#[async_trait]
pub trait InboundRepository: Send + Sync {
	fn deployment(&self) -> &DeploymentId;
	/// `[deployment] federation`: whether this Deployment federates with others.
	fn federation_enabled(&self) -> bool;
	/// Whether this Deployment holds a Deployment Key.
	fn holds_key(&self) -> bool;
	/// Seconds since the epoch.
	fn now(&self) -> u64;
	/// The Tenant that holds `node`, from the Deployment-wide record of Node
	/// names. Opens no Tenant scope.
	async fn node_tenant(&self, node: &NodeName) -> Result<Option<TenantName>>;
	/// Whether `tenant` has no deletion record.
	async fn tenant_live(&self, tenant: &TenantName) -> Result<bool>;
	/// Permanent name ownership is not live admission.
	async fn node_live(&self, tenant: &TenantName, node: &NodeName) -> Result<bool>;
	/// An authenticated request may finish only an exact durable obligation.
	/// Implementations bind the original caller, receiver and immutable work.
	async fn closed_node_request(
		&self,
		tenant: &TenantName,
		receiver: &NodeAddress,
		sender: &NodeAddress,
		subject: &str,
		request: &crate::federation::inbound::RequestDigest<'_>,
	) -> Result<bool>;
	/// The registration at `receiver` for `sender`, read in `tenant`'s scope.
	async fn registration(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
	) -> Result<Option<NodeRegistration>>;
	/// The enabled Peer Mapping of `(receiver, sender, subject)`.
	async fn mapping(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>>;
	/// Whether `subject` is an enabled subject of the Tenant's policy that may
	/// `node.use` the receiving Node.
	async fn subject_may_use(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		subject: &str,
	) -> Result<bool>;
	/// The enabled Peer Graph Grant of `identity` of `viewer` at `tenant`.
	async fn viewer_grant(
		&self,
		tenant: &TenantName,
		viewer: &DeploymentId,
		identity: Uuid,
	) -> Result<Option<ViewerGrant>>;
	/// Authentication material only. This read does not establish enabled
	/// authority; admission must separately check the registration or grant.
	async fn receiving_keys(&self, owner: &ReceivingKeyOwner) -> Result<Option<ReceivingKeys>>;
	/// Atomically advances only this owner's authentication material and its
	/// keys revision. Match BOTH revisions, enabled authority and non-quarantine
	/// under the existing Tenant/Node authority locks, rechecking live Tenant and
	/// receiving Node. Preserve permissions, authorization revision and cursors.
	/// False means admission must fail closed or fully reload and revalidate;
	/// a stale snapshot must never be written unconditionally after a conflict.
	async fn advance_keys(
		&self,
		owner: &ReceivingKeyOwner,
		expected_keys_revision: i64,
		expected_authorization_revision: i64,
		keys: &TrustedDeploymentKeys,
	) -> Result<bool>;
}
