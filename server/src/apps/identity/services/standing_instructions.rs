//! Tenant Instructions and Node Instructions (#1010). Only a person's browser
//! session reaches these: `administrator` refuses Subject Credentials, so an
//! Agent's chain never writes them, even when its root person could.
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	models::{AdministrationActor, AdministrationEvent},
	oidc::BrowserOrigin,
	repositories::standing_instructions::{self as repository, Target},
	serializers::standing_instructions::{Instructions, Update},
};
use crate::{Result, authorization::identity::Actor};
use aidash_domain::identity::tenant_administration::{
	NODE_ADMINISTER, NODE_READ, TENANT_INSTRUCTIONS_UPDATE,
};

impl TenantAdministrations {
	/// Every member of the Tenant reads its Tenant Instructions, which every
	/// Agent of the Tenant receives anyway; writing needs
	/// `tenant.instructions.update`.
	pub(crate) async fn tenant_instructions(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		update: Option<Update>,
	) -> Result<Instructions> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				if let Some(update) = update {
					administrator
						.locked_for(tx, TENANT_INSTRUCTIONS_UPDATE, true)
						.await?;
					let by = AdministrationActor::TenantAdministrator(administrator.clone());
					repository::set(
						tx,
						&tenant,
						Target::Tenant,
						&update.text,
						update.expected_revision,
						&by.decision_actor(),
					)
					.await?;
					by.record(
						tx,
						AdministrationEvent {
							action: TENANT_INSTRUCTIONS_UPDATE,
							identity: None,
							registration: None,
							mapping: None,
							tenant: Some(&tenant),
							subject: None,
							invitation: None,
						},
					)
					.await?;
				}
				Ok(repository::get(tx, &tenant, Target::Tenant).await?.into())
			})
			.await;
		self.finish(&administrator, result).await
	}

	/// Reading a Node's Instructions needs `node.read` on that Node, as listing
	/// it does; writing needs `node.administer` on it.
	pub(crate) async fn node_instructions(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		update: Option<Update>,
	) -> Result<Instructions> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = administrator.lock(tx, false).await?;
				let current =
					crate::apps::identity::repositories::nodes::get_live(tx, &tenant, &node)
						.await?;
				let attributes =
					serde_json::json!({"label": current.label, "default": current.default});
				administrator
					.require(
						tx,
						&snapshot,
						if update.is_some() {
							NODE_ADMINISTER
						} else {
							NODE_READ
						},
						&node,
						attributes,
					)
					.await?;
				if let Some(update) = update {
					let by = AdministrationActor::TenantAdministrator(administrator.clone());
					repository::set(
						tx,
						&tenant,
						Target::Node(&node),
						&update.text,
						update.expected_revision,
						&by.decision_actor(),
					)
					.await?;
					by.record_node(
						tx,
						AdministrationEvent {
							action: NODE_ADMINISTER,
							identity: None,
							registration: None,
							mapping: None,
							tenant: Some(&tenant),
							subject: None,
							invitation: None,
						},
						&node,
					)
					.await?;
				}
				Ok(repository::get(tx, &tenant, Target::Node(&node))
					.await?
					.into())
			})
			.await;
		self.finish(&administrator, result).await
	}
}
