//! The Deployment's answers to `accept` (ADR 0064): which Node names a Tenant,
//! whether that Tenant is deleted, and what its Node Registrations, Peer
//! Mappings, policy and Peer Graph Grants say.
use crate::apps::identity::models::AuthorizationBundle;
use crate::apps::identity::repositories::{
	node_registrations as registrations, nodes, tenant_deletions,
};
use crate::federation::Federation;
use aidash_application::{
	Result,
	ports::federation::inbound::{
		InboundRepository, ReceivingKeyOwner, ReceivingKeys, ViewerGrant,
	},
	tenancy::TenancyScope,
};
use aidash_domain::{
	identity::{peer_mapping::Mapping, tenant_administration::NODE_USE},
	nodes::{
		DeploymentId, NodeAddress, NodeName,
		registration::{NodeRegistration, PinnedKey},
	},
	policy::{Evaluation, Resource},
	tenancy::TenantName,
};
use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;

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
		Ok(self
			.store
			.pool
			.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
				let snapshot =
					AuthorizationBundle::lock_snapshot(tx, tenant.as_str(), false).await?;
				if !aidash_domain::identity::authority::enabled(&snapshot.bundle, subject) {
					return Ok(false);
				}
				let node = nodes::get(tx, tenant.as_str(), receiver.as_str()).await?;
				let evaluation = Evaluation {
					subject: subject.to_owned(),
					action: NODE_USE.into(),
					resource: Resource {
						tenant: tenant.to_string(),
						kind: "node".into(),
						id: receiver.to_string(),
						attributes: json!({"label": node.label, "default": node.default}),
					},
					environment: json!({"transport": "federation"}),
				};
				Ok(snapshot.bundle.evaluate(&evaluation).allowed)
			})
			.await?)
	}

	async fn viewer_grant(
		&self,
		tenant: &TenantName,
		viewer: &DeploymentId,
		identity: Uuid,
	) -> Result<Option<ViewerGrant>> {
		let row = crate::apps::identity::repositories::graph::persistence::viewer_grant(
			self,
			tenant.as_str(),
			viewer.as_str(),
			identity,
		)
		.await?;
		row.map(|(key_id, public_key_pem, revision)| {
			Ok(ViewerGrant {
				key: PinnedKey {
					key_id: aidash_domain::nodes::DeploymentKeyId::parse(&key_id)?,
					public_key_pem,
				},
				revision,
			})
		})
		.transpose()
	}
	async fn receiving_keys(&self, owner: &ReceivingKeyOwner) -> Result<Option<ReceivingKeys>> {
		use crate::apps::identity::repositories::receiving_keys;
		Ok(self
			.store
			.pool
			.atomic(
				&TenancyScope::Tenant(receiving_keys::tenant(owner).clone()),
				async |tx| receiving_keys::get(tx, owner).await,
			)
			.await?)
	}

	async fn advance_keys(
		&self,
		owner: &ReceivingKeyOwner,
		expected_keys_revision: i64,
		expected_authorization_revision: i64,
		keys: &aidash_domain::nodes::keys::TrustedDeploymentKeys,
	) -> Result<bool> {
		use crate::apps::identity::repositories::receiving_keys;
		let tenant = receiving_keys::tenant(owner);
		Ok(self
			.store
			.pool
			.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
				// Deletion takes exclusive policy then capacity locks in this order.
				let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant.as_str(), false).await;
				match snapshot {
					Err(crate::Error::TenantDeleted | crate::Error::NotFound(_)) => {
						return Ok(false);
					}
					other => {
						other?;
					}
				}
				if let ReceivingKeyOwner::Registration { node, .. } = owner {
					nodes::lock_tenant(tx, tenant.as_str()).await?;
					match nodes::get_live(tx, tenant.as_str(), node.as_str()).await {
						Err(crate::Error::NotFound(_)) => return Ok(false),
						other => {
							other?;
						}
					}
				}
				receiving_keys::advance(
					tx,
					owner,
					expected_keys_revision,
					expected_authorization_revision,
					keys,
				)
				.await
			})
			.await?)
	}
}
