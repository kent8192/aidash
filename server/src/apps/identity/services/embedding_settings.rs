use super::tenant_administration::TenantAdministrations;
use crate::{
	Result,
	apps::identity::{
		models::{AdministrationActor, AdministrationEvent},
		oidc::BrowserOrigin,
		repositories::embedding_settings::{self as repository, Response, Update},
	},
	authorization::identity::Actor,
};
use aidash_domain::identity::tenancy::TENANT_UPDATE;

impl TenantAdministrations {
	pub(crate) async fn embedding_settings(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		update: Option<Update>,
	) -> Result<Response> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let estimate = if let Some(update) = update {
					administrator.locked_for(tx, TENANT_UPDATE, true).await?;
					let confirmed = update.confirm;
					let estimate = repository::change(tx, &tenant, update).await?;
					if confirmed {
						AdministrationActor::TenantAdministrator(administrator.clone())
							.record(
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
					estimate
				} else {
					None
				};
				Ok(Response {
					settings: repository::read(tx, &tenant).await?,
					embeddings: repository::embeddings(tx, &tenant).await?,
					estimate,
				})
			})
			.await;
		self.finish(&administrator, result).await
	}
}
