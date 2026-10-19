//! Profile ORM operations retain their original transactions and revision fence.
use crate::apps::registry::workbench::models::{AgentDraft, AgentTestProfile};
use crate::{authorization::identity::Actor, federation::Federation};
use aidash_application::tenancy::TenancyScope;
use aidash_application::{
	Result,
	ports::registry::workbench::profile::{
		ProfileConfiguration, ProfileDraftScope, ProfileRepository,
	},
};
use aidash_domain::{
	identity::Principal,
	registry::{
		EntityRef, Entry,
		workbench::{
			Draft,
			profile::{ProfileInput, TestProfile},
		},
	},
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) runtime: &'a Federation,
	pub(crate) actor: Actor,
}
/// Test endpoints share the Deployment's permitted private origins with
/// Keyless endpoints until #376 decides whether tools keep a list of their own.
pub(crate) struct Configuration {
	pub(crate) keyless: aidash_application::provider_access::KeylessAccess,
}
impl ProfileConfiguration for Configuration {
	fn destination(&self, endpoint: &str) -> Result<aidash_domain::keyless::Destination> {
		self.keyless.destination(endpoint)
	}
}
struct Scope {
	configured: bool,
	node: String,
	tx: crate::database::native::Transaction,
	actor: Actor,
}
#[async_trait]
impl ProfileRepository for Repository<'_> {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
	}
	async fn begin_draft(&self) -> Result<Box<dyn ProfileDraftScope + '_>> {
		Ok(Box::new(Scope {
			configured: self.runtime.store.provider_credentials.is_some(),
			node: self.runtime.config.deployment.to_string(),
			tx: self
				.runtime
				.store
				.pool
				.begin(&self.actor.tenancy()?)
				.await?,
			actor: self.actor.clone(),
		}))
	}
	async fn page(&self, tenant: &str) -> Result<Vec<TestProfile>> {
		self.runtime
			.store
			.pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				AgentTestProfile::page(tx, tenant).await
			})
			.await
			.map_err(Into::into)
	}
	async fn administered(&self, tenant: &str) -> Result<Vec<TestProfile>> {
		let pool = &self.runtime.store.pool;
		let actions = self.tenant_actions();
		let result = pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				actions
					.require(
						&mut *tx,
						pool.dashboard_policy().as_ref(),
						tenant,
						aidash_domain::identity::tenant_administration::TEST_PROFILE_UPDATE,
						aidash_domain::identity::tenant_administration::COLLECTION,
					)
					.await?;
				AgentTestProfile::page(tx, tenant).await
			})
			.await;
		actions.record_denial(pool).await?;
		result.map_err(Into::into)
	}
	async fn authorize_update(&self, tenant: &str, id: &str) -> Result<()> {
		let pool = &self.runtime.store.pool;
		let actions = self.tenant_actions();
		let mut check = pool.begin(&TenancyScope::tenant(tenant)?).await?;
		let result = actions
			.require(
				&mut check,
				pool.dashboard_policy().as_ref(),
				tenant,
				aidash_domain::identity::tenant_administration::TEST_PROFILE_UPDATE,
				id,
			)
			.await;
		// Nothing is kept: `save` decides again with the write and keeps that
		// decision; a denial is audited after the rollback.
		check.rollback().await?;
		actions.record_denial(pool).await?;
		result.map_err(Into::into)
	}
	async fn definition(&self, tenant: &str, reference: &EntityRef) -> Result<Entry> {
		self.runtime
			.registry
			.get(
				&aidash_domain::tenancy::TenantName::new(tenant)?,
				&reference.id,
				&reference.version,
			)
			.await
			.map_err(Into::into)
	}
	async fn save(&self, tenant: &str, id: &str, input: &ProfileInput) -> Result<TestProfile> {
		let pool = &self.runtime.store.pool;
		let actions = self.tenant_actions();
		let result = pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				actions
					.require(
						&mut *tx,
						pool.dashboard_policy().as_ref(),
						tenant,
						aidash_domain::identity::tenant_administration::TEST_PROFILE_UPDATE,
						id,
					)
					.await?;
				AgentTestProfile::save(tx, tenant, id, input).await
			})
			.await;
		actions.record_denial(pool).await?;
		result.map_err(Into::into)
	}
}
impl Repository<'_> {
	/// A denial is audited after `pool.atomic` has rolled its transaction back.
	fn tenant_actions(&self) -> super::authority::TenantActions {
		super::authority::TenantActions::new(
			self.actor.clone(),
			self.runtime.config.deployment.to_string(),
		)
	}
}
#[async_trait]
impl ProfileDraftScope for Scope {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> aidash_application::Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let node = self.node.clone();
		let configured = self.configured;
		crate::apps::registry::repositories::bindings::preview(
			&mut *self.tx,
			&node,
			entry,
			configured,
		)
		.await
	}
	async fn draft(&mut self, id: Uuid) -> Result<Draft> {
		Ok(AgentDraft::read(&mut self.tx, id, false).await?.into())
	}
	async fn authorize(&mut self, draft: &Draft, action: &str, shares: bool) -> Result<()> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::authorize(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &self.actor, policy),
			draft,
			action,
			shares,
		)
		.await
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Box::new(self.tx)
			.commit()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}
