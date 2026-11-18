//! A Tenant's execution settings (#1009). Every member reads them; writing
//! needs `tenant.update` from a person's browser session, so an Agent's chain
//! never changes them.
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	models::{AdministrationActor, AdministrationEvent},
	oidc::BrowserOrigin,
	repositories::execution_settings as repository,
	serializers::execution_settings::{TenantExecutionSettings, TenantExecutionSettingsUpdate},
};
use crate::{Result, authorization::identity::Actor};
use aidash_domain::identity::tenancy::TENANT_UPDATE;

impl TenantAdministrations {
	pub(crate) async fn execution_settings(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		update: Option<TenantExecutionSettingsUpdate>,
	) -> Result<TenantExecutionSettings> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				if let Some(update) = update {
					administrator.locked_for(tx, TENANT_UPDATE, true).await?;
					let by = AdministrationActor::TenantAdministrator(administrator.clone());
					repository::set(
						tx,
						&tenant,
						update.lease_recovery_limit,
						update.expected_revision,
						&by.decision_actor(),
					)
					.await?;
					by.record(
						tx,
						AdministrationEvent {
							action: TENANT_UPDATE,
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
				Ok(repository::get(tx, &tenant).await?.into())
			})
			.await;
		self.finish(&administrator, result).await
	}
}
