//! Browser-mapped Tenant Administrators manage the policy in their authorization transaction.
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	models::{AdministrationActor, AdministrationEvent},
	oidc::BrowserOrigin,
	repositories::data_protection as repository,
	serializers::data_protection::{Impact, Settings},
};
use crate::{Error, Result, authorization::identity::Actor};
use aidash_domain::{
	data_protection::DataProtection,
	identity::tenant_administration::{POLICY_READ, POLICY_UPDATE},
};

impl TenantAdministrations {
	pub(crate) async fn data_protection(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		update: Option<DataProtection>,
	) -> Result<Settings> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let floor = self.runtime.store.data_protection_floor;
		if let Some(policy) = update {
			policy.validate()?;
			if policy.level < floor {
				return Err(Error::Invalid(
					"Data Protection Level is below the Deployment floor".into(),
				));
			}
		}
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
				if let Some(policy) = update {
					let by = AdministrationActor::TenantAdministrator(administrator.clone());
					repository::save(tx, &tenant, policy, &by.decision_actor()).await?;
					by.record(
						tx,
						AdministrationEvent {
							action: "tenant.data_protection.update",
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
				let policy = repository::stored(tx, &tenant).await?;
				Ok(Settings {
					policy,
					effective: policy.with_floor(floor),
					deployment_floor: floor,
				})
			})
			.await;
		self.finish(&administrator, result).await
	}
	pub(crate) async fn data_protection_impact(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		policy: DataProtection,
	) -> Result<Impact> {
		policy.validate()?;
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				administrator.locked_for(tx, POLICY_READ, false).await?;
				repository::impact(
					tx,
					&self.runtime.store,
					&tenant,
					policy.with_floor(self.runtime.store.data_protection_floor),
				)
				.await
			})
			.await;
		self.finish(&administrator, result).await
	}
}
