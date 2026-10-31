//! The Deployment's answers to `accept` (ADR 0064): which Node names a Tenant,
//! whether that Tenant is deleted, and what its Node Registrations, Peer
//! Mappings and policy say.
use crate::apps::identity::repositories::{
	node_registrations as registrations, nodes, tenant_deletions,
};
use crate::apps::identity::services::receiving_keys;
use crate::federation::Federation;
use aidash_application::{
	Result,
	ports::federation::inbound::{InboundRepository, ReceivingKeyOwner, ReceivingKeys},
	tenancy::TenancyScope,
};
use aidash_domain::{
	identity::peer_mapping::Mapping,
	nodes::{DeploymentId, NodeAddress, NodeName, registration::NodeRegistration},
	tenancy::TenantName,
};
use async_trait::async_trait;

#[async_trait]
impl InboundRepository for Federation {
	fn deployment(&self) -> &DeploymentId {
		&self.config.deployment
	}

	fn federation_enabled(&self) -> bool {
		self.config.federation
	}

	fn holds_key(&self) -> bool {
		self.config.deployment_key.is_some()
	}

	fn now(&self) -> u64 {
		chrono::Utc::now().timestamp().max(0) as u64
	}

	async fn node_tenant(&self, node: &NodeName) -> Result<Option<TenantName>> {
		let holder = self
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				registrations::name_tenant(tx, node.as_str()).await
			})
			.await?;
		holder.map(TenantName::new).transpose().map_err(Into::into)
	}

	async fn tenant_live(&self, tenant: &TenantName) -> Result<bool> {
		Ok(!self
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				tenant_deletions::is_deleted(tx, tenant.as_str()).await
			})
			.await?)
	}

	async fn node_live(&self, tenant: &TenantName, node: &NodeName) -> Result<bool> {
		Ok(self
			.store
			.pool
			.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
				nodes::is_live(tx, tenant.as_str(), node.as_str()).await
			})
			.await?)
	}
	async fn closed_node_request(
		&self,
		tenant: &TenantName,
		receiver: &NodeAddress,
		sender: &NodeAddress,
		subject: &str,
		request: &aidash_application::federation::inbound::RequestDigest<'_>,
	) -> Result<bool> {
		Ok(self
			.store
			.pool
			.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
				super::super::repositories::routes::closed_node_request_in(
					tx,
					tenant.as_str(),
					receiver,
					sender,
					subject,
					request,
				)
				.await
			})
			.await?)
	}
	async fn registration(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
	) -> Result<Option<NodeRegistration>> {
		Ok(self
			.store
			.pool
			.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
				registrations::get(tx, tenant.as_str(), receiver.as_str(), sender).await
			})
			.await?)
	}

	async fn mapping(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>> {
		use aidash_application::ports::authorization::peer::mappings::MappingRepository;
		crate::bootstrap::peer_mapping_repository(self)
			.resolved(tenant, receiver, sender, subject)
			.await
	}

	async fn subject_may_use(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		subject: &str,
	) -> Result<bool> {
		Ok(receiving_keys::subject_may_use(&self.store.pool, tenant, receiver, subject).await?)
	}

	async fn receiving_keys(&self, owner: &ReceivingKeyOwner) -> Result<Option<ReceivingKeys>> {
		Ok(receiving_keys::get(&self.store.pool, owner).await?)
	}

	async fn advance_keys(
		&self,
		owner: &ReceivingKeyOwner,
		expected_keys_revision: i64,
		expected_authorization_revision: i64,
		keys: &aidash_domain::nodes::keys::TrustedDeploymentKeys,
	) -> Result<bool> {
		Ok(receiving_keys::advance(
			&self.store.pool,
			owner,
			expected_keys_revision,
			expected_authorization_revision,
			keys,
		)
		.await?)
	}
	async fn advance_recovery_keys(
		&self,
		owner: &ReceivingKeyOwner,
		expected_keys_revision: i64,
		expected_authorization_revision: i64,
		keys: &aidash_domain::nodes::keys::TrustedDeploymentKeys,
		subject: &str,
		request: &aidash_application::federation::inbound::RequestDigest<'_>,
	) -> Result<bool> {
		let proof = DurableRequest {
			deployment: &self.config.deployment,
			subject,
			request,
		};
		Ok(receiving_keys::advance_recovery(
			&self.store.pool,
			owner,
			expected_keys_revision,
			expected_authorization_revision,
			keys,
			&proof,
		)
		.await?)
	}
}

/// Federation owns the durable protocol proof; Identity owns the transaction.
struct DurableRequest<'a, 'body> {
	deployment: &'a DeploymentId,
	subject: &'a str,
	request: &'a aidash_application::federation::inbound::RequestDigest<'body>,
}
#[async_trait]
impl receiving_keys::RecoveryProof for DurableRequest<'_, '_> {
	async fn validate(
		&self,
		tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
		owner: &ReceivingKeyOwner,
	) -> crate::Result<bool> {
		let ReceivingKeyOwner::Registration {
			tenant,
			node,
			source_node,
		} = owner;
		let receiver = self.deployment.node(node.clone());
		super::super::repositories::routes::closed_node_request_in(
			tx,
			tenant.as_str(),
			&receiver,
			source_node,
			self.subject,
			self.request,
		)
		.await
	}
}
