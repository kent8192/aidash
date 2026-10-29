//! Manual receiving trust uses the existing record's management authority,
//! which is Tenant administration (#484, ADR 0102).
//! Current authority also covers exact retained recovery registrations after
//! finite Node catalog expiry; neither missing Nodes nor history imply access.
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	models::{AdministrationActor, AdministrationEvent},
	oidc::BrowserOrigin,
	repositories::{manual_keys as records, node_recovery, node_registrations, nodes},
	serializers::manual_keys::{KeyChange, ReceivingKeyStatus},
};
use crate::authorization::identity::Actor;
use crate::federation::Federation;
use crate::{Error, Result};
use aidash_application::ports::federation::inbound::ReceivingKeyOwner;
use aidash_domain::{
	identity::tenant_administration as rules,
	nodes::{DeploymentId, NodeAddress, NodeName},
	tenancy::TenantName,
};
use reinhardt::db::backends::TransactionExecutor;
use serde_json::json;

pub struct ManualKeys {
	runtime: Federation,
}
#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> ManualKeys {
	ManualKeys { runtime }
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
			// Registration records are Tenant administration (#484, ADR 0102).
			Actor::DeploymentAdministrator => Err(Error::Forbidden),
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
					registration_owner(tx, &f.config.deployment, &tenant, &node, &source).await?;
					apply(tx, &owner, change.as_ref(), &AdministrationActor::TenantAdministrator(administrator.clone())).await
				}).await
			}).await,
		}
	}
}

/// Only this manual control may use an exact retained dependency when the
/// catalog has expired. The caller retains current policy and capacity locks.
async fn registration_owner(
	tx: &mut dyn TransactionExecutor,
	deployment: &DeploymentId,
	tenant: &str,
	node: &NodeName,
	source: &NodeAddress,
) -> Result<()> {
	match nodes::get(tx, tenant, node.as_str()).await {
		Ok(_) => Ok(()),
		Err(missing @ Error::NotFound(_)) => {
			if node_registrations::name_tenant(tx, node.as_str())
				.await?
				.as_deref() != Some(tenant)
			{
				return Err(missing);
			}
			let required =
				node_recovery::registration_sources_in(tx, deployment, tenant, node.as_str())
					.await?;
			if required.contains(&source.to_string()) {
				Ok(())
			} else {
				Err(missing)
			}
		}
		Err(error) => Err(error),
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
		let ReceivingKeyOwner::Registration {
			tenant,
			node,
			source_node,
		} = owner;
		let subject = source_node.to_string();
		let event = AdministrationEvent {
			action: change.action_name(),
			identity: None,
			registration: None,
			mapping: None,
			tenant: Some(tenant.as_str()),
			subject: Some(&subject),
			invitation: None,
		};
		by.record_node(tx, event, node.as_str()).await?;
	}
	Ok(status)
}
