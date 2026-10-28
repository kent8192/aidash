//! What accepting an inbound Federation request needs to know about the
//! Deployment and its Tenants (ADR 0064).
use crate::Result;
use aidash_domain::{
	identity::peer_mapping::Mapping,
	nodes::{DeploymentId, NodeAddress, NodeName, registration::NodeRegistration},
	tenancy::TenantName,
};
use async_trait::async_trait;

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
}
