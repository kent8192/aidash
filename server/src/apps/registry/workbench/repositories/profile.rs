//! Profile ORM operations retain their original transactions and revision fence.
use crate::apps::registry::workbench::models::{AgentDraft, AgentTestProfile};
use crate::{authorization::identity::Actor, federation::Federation};
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
pub(crate) struct Configuration;
impl ProfileConfiguration for Configuration {
	fn require_secret(&self, name: &str) -> Result<()> {
		crate::config::secret(name).map(|_| ()).map_err(Into::into)
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
			node: self.runtime.config.node_id.clone(),
			tx: crate::database::native::begin(&self.runtime.store.pool).await?,
			actor: self.actor.clone(),
		}))
	}
	async fn page(&self, tenant: &str) -> Result<Vec<TestProfile>> {
		let lease = self.runtime.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| AgentTestProfile::page(tx, tenant).await)
			.await
			.map_err(Into::into)
	}
	async fn definition(&self, reference: &EntityRef) -> Result<Entry> {
		self.runtime
			.registry
			.get(&reference.id, &reference.version)
			.await
			.map_err(Into::into)
	}
	async fn save(&self, tenant: &str, id: &str, input: &ProfileInput) -> Result<TestProfile> {
		let lease = self.runtime.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| AgentTestProfile::save(tx, tenant, id, input).await)
			.await
			.map_err(Into::into)
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
