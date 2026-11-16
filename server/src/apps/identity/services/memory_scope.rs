//! Tenant Administrators choose whether the Agent may recall across Workspaces.
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	models::{AdministrationActor, AdministrationEvent},
	oidc::BrowserOrigin,
	serializers::memory_scope::{Settings, Update},
};
use crate::{Result, authorization::identity::Actor};
use aidash_domain::identity::tenant_administration::{POLICY_READ, POLICY_UPDATE};

impl TenantAdministrations {
	pub(crate) async fn memory_scope(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		update: Option<Update>,
	) -> Result<Settings> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				administrator
					.locked_for(
						tx,
						if update.is_some() {
							POLICY_UPDATE
						} else {
							POLICY_READ
						},
						update.is_some(),
					)
					.await?;
				if let Some(update) = update {
					crate::apps::knowledge::repositories::sharing::set(
						tx,
						&tenant,
						update.scope,
						update.expected_revision,
					)
					.await?;
					let by = AdministrationActor::TenantAdministrator(administrator.clone());
					by.record(
						tx,
						AdministrationEvent {
							action: "tenant.memory_scope.update",
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
				let (scope, revision) =
					crate::apps::knowledge::repositories::sharing::get(tx, &tenant).await?;
				Ok(Settings { scope, revision })
			})
			.await;
		self.finish(&administrator, result).await
	}
}
