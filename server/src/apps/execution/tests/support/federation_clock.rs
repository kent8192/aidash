use aidash_application::{
	federation::inbound::RequestDigest,
	ports::federation::inbound::{
		InboundRepository, ReceivingKeyOwner, ReceivingKeys, ViewerGrant,
	},
};
use aidash_domain::{
	identity::peer_mapping::Mapping,
	nodes::{
		DeploymentId, NodeAddress, NodeName, keys::TrustedDeploymentKeys,
		registration::NodeRegistration,
	},
	tenancy::TenantName,
};
use aidash_server::federation::Federation;
use async_trait::async_trait;
use uuid::Uuid;

/// Only time is virtual. All authority/key reads and writes delegate to the
/// restarted production PostgreSQL adapter, with no production test seam.
pub struct Clock<'a>(pub &'a Federation, pub u64);
#[async_trait]
impl InboundRepository for Clock<'_> {
	fn deployment(&self) -> &DeploymentId {
		self.0.deployment()
	}
	fn federation_enabled(&self) -> bool {
		self.0.federation_enabled()
	}
	fn holds_key(&self) -> bool {
		self.0.holds_key()
	}
	fn now(&self) -> u64 {
		self.1
	}
	async fn node_tenant(&self, node: &NodeName) -> aidash_application::Result<Option<TenantName>> {
		self.0.node_tenant(node).await
	}
	async fn tenant_live(&self, tenant: &TenantName) -> aidash_application::Result<bool> {
		self.0.tenant_live(tenant).await
	}
	async fn node_live(
		&self,
		tenant: &TenantName,
		node: &NodeName,
	) -> aidash_application::Result<bool> {
		self.0.node_live(tenant, node).await
	}
	async fn closed_node_request(
		&self,
		tenant: &TenantName,
		receiver: &NodeAddress,
		sender: &NodeAddress,
		subject: &str,
		request: &RequestDigest<'_>,
	) -> aidash_application::Result<bool> {
		self.0
			.closed_node_request(tenant, receiver, sender, subject, request)
			.await
	}

	async fn registration(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
	) -> aidash_application::Result<Option<NodeRegistration>> {
		self.0.registration(tenant, receiver, sender).await
	}
	async fn mapping(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
		subject: &str,
	) -> aidash_application::Result<Option<Mapping>> {
		self.0.mapping(tenant, receiver, sender, subject).await
	}
	async fn subject_may_use(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		subject: &str,
	) -> aidash_application::Result<bool> {
		self.0.subject_may_use(tenant, receiver, subject).await
	}
	async fn viewer_grant(
		&self,
		tenant: &TenantName,
		viewer: &DeploymentId,
		identity: Uuid,
	) -> aidash_application::Result<Option<ViewerGrant>> {
		self.0.viewer_grant(tenant, viewer, identity).await
	}
	async fn receiving_keys(
		&self,
		owner: &ReceivingKeyOwner,
	) -> aidash_application::Result<Option<ReceivingKeys>> {
		self.0.receiving_keys(owner).await
	}
	async fn advance_keys(
		&self,
		owner: &ReceivingKeyOwner,
		kr: i64,
		ar: i64,
		keys: &TrustedDeploymentKeys,
	) -> aidash_application::Result<bool> {
		self.0.advance_keys(owner, kr, ar, keys).await
	}
	async fn advance_recovery_keys(
		&self,
		owner: &ReceivingKeyOwner,
		kr: i64,
		ar: i64,
		keys: &TrustedDeploymentKeys,
		subject: &str,
		request: &RequestDigest<'_>,
	) -> aidash_application::Result<bool> {
		self.0
			.advance_recovery_keys(owner, kr, ar, keys, subject, request)
			.await
	}
}
