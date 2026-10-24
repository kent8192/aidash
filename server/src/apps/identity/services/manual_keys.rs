//! Manual receiving trust uses the existing record's management authority.
//! Deleted Node catalog records remain manageable here; absent catalog records
//! require the exact retained-journal helper and are deliberately not inferred.
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	models::{AdministrationActor, AdministrationEvent},
	oidc::BrowserOrigin,
	repositories::{manual_keys as records, nodes},
	serializers::manual_keys::{GraphKeySelector, KeyChange, ReceivingKeyStatus},
};
use crate::authorization::identity::Actor;
use crate::federation::Federation;
use crate::{Error, Result};
use aidash_application::{ports::federation::inbound::ReceivingKeyOwner, tenancy::TenancyScope};
use aidash_domain::{
	identity::tenant_administration as rules,
	nodes::{DeploymentId, NodeAddress, NodeName},
	tenancy::TenantName,
};
use reinhardt::db::{backends::TransactionExecutor, orm::AtomicTransaction};
use serde_json::json;

pub struct ManualKeys {
	runtime: Federation,
}
#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> ManualKeys {
	ManualKeys { runtime }
}

async fn deployment_actor(
	tx: &mut AtomicTransaction,
	origin: Option<&BrowserOrigin>,
) -> Result<AdministrationActor> {
	if let Some(origin) = origin {
		origin.require_deployment_administrator(tx, true).await?;
		Ok(AdministrationActor::DeploymentAdministrator {
			identity: origin.identity_id,
		})
	} else {
		Ok(AdministrationActor::DeploymentAdministratorBearer)
	}
}

impl ManualKeys {
	pub(crate) async fn registration(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		source: String,
		change: Option<KeyChange>,
	) -> Result<ReceivingKeyStatus> {
		let node = NodeName::parse(&node)?;
		let source = NodeAddress::parse(&source)?;
		let owner = ReceivingKeyOwner::Registration {
			tenant: TenantName::new(&tenant)?,
			node: node.clone(),
			source_node: source.clone(),
		};
		let f = &self.runtime;
		match actor {
			Actor::DeploymentAdministrator => f.store.control_pool.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
				super::remote::deployment_administrator::tenant(tx, &tenant).await?;
				let by = deployment_actor(tx, origin.as_ref()).await?;
				nodes::lock_tenant(tx, &tenant).await?;
				nodes::get(tx, &tenant, node.as_str()).await?;
				apply(tx, &owner, change.as_ref(), &by).await
			}).await,
			actor => TenantAdministrations { runtime: f.clone() }.run(actor, origin, &tenant, async |administrator| {
				f.store.control_pool.atomic(&administrator.scope()?, async |tx| {
					let snapshot = administrator.lock(tx, false).await?;
					let writing = change.is_some();
					let resource = if writing { source.to_string() } else { node.to_string() };
					let attributes = if writing {
						json!({"node":node.as_str(),"source_node":source.to_string(),"source_deployment":source.deployment.as_str()})
					} else { json!({"node":node.as_str()}) };
					administrator.require(tx, &snapshot,
						if writing { rules::NODE_REGISTRATION_UPDATE } else { rules::NODE_REGISTRATION_READ },
						&resource, attributes).await?;
					nodes::lock_tenant(tx, &tenant).await?;
					nodes::get(tx, &tenant, node.as_str()).await?;
					apply(tx, &owner, change.as_ref(), &AdministrationActor::TenantAdministrator(administrator.clone())).await
				}).await
			}).await,
		}
	}

	pub(crate) async fn graph(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		selector: GraphKeySelector,
		change: Option<KeyChange>,
	) -> Result<ReceivingKeyStatus> {
		if !matches!(actor, Actor::DeploymentAdministrator) {
			return Err(Error::Forbidden);
		}
		let owner = ReceivingKeyOwner::ViewerGrant {
			tenant: TenantName::new(&tenant)?,
			viewer: DeploymentId::parse(&selector.source_deployment)?,
			identity: selector.source_identity,
		};
		self.runtime
			.store
			.control_pool
			.atomic(&TenancyScope::tenant(&tenant)?, async |tx| {
				super::remote::deployment_administrator::tenant(tx, &tenant).await?;
				let by = deployment_actor(tx, origin.as_ref()).await?;
				apply(tx, &owner, change.as_ref(), &by).await
			})
			.await
	}
}

async fn apply(
	tx: &mut dyn TransactionExecutor,
	owner: &ReceivingKeyOwner,
	change: Option<&KeyChange>,
	by: &AdministrationActor,
) -> Result<ReceivingKeyStatus> {
	let status = records::control(tx, owner, change).await?;
	if let Some(change) = change {
		let (tenant, subject, node) = match owner {
			ReceivingKeyOwner::Registration {
				tenant,
				node,
				source_node,
			} => (tenant, source_node.to_string(), Some(node.as_str())),
			ReceivingKeyOwner::ViewerGrant {
				tenant,
				viewer,
				identity,
			} => (tenant, format!("{viewer}/oidc:{identity}"), None),
		};
		let event = AdministrationEvent {
			action: change.action_name(node.is_some()),
			identity: None,
			registration: None,
			mapping: None,
			tenant: Some(tenant.as_str()),
			subject: Some(&subject),
			invitation: None,
		};
		if let Some(node) = node {
			by.record_node(tx, event, node).await?;
		} else {
			by.record(tx, event).await?;
		}
	}
	Ok(status)
}
