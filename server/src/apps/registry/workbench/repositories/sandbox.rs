//! Native sandbox management retains current identity, tenant limits and atomic stop predicates.
use crate::apps::registry::workbench::models::{AgentDraft, AgentTestLimit, AgentTestSession};
use crate::{authorization::identity::Actor, store::Store};
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::{SandboxRepository, SandboxScope},
};
use aidash_domain::{
	identity::Principal,
	registry::workbench::{
		Draft,
		sandbox::{TestLimits, TestSession},
	},
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use uuid::Uuid;
pub(crate) struct Repository {
	pub(crate) store: Store,
	pub(crate) node_id: String,
	pub(crate) actor: Actor,
}
struct Scope {
	node_id: String,
	tx: crate::database::native::Transaction,
	actor: Actor,
}
#[async_trait]
impl SandboxRepository for Repository {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
	}
	fn validate_limit_fields(&self, limits: &TestLimits) -> Result<()> {
		let native: crate::apps::registry::workbench::serializers::test::TestLimits =
			limits.clone().into();
		crate::http::validate(&native).map_err(Into::into)
	}
	async fn begin(&self) -> Result<Box<dyn SandboxScope + '_>> {
		Ok(Box::new(Scope {
			tx: crate::database::native::begin(&self.store.pool).await?,
			actor: self.actor.clone(),
			node_id: self.node_id.clone(),
		}))
	}
	async fn purge(&self) -> Result<u64> {
		let lease = self.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| AgentTestSession::purge(tx).await)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl SandboxScope for Scope {
	async fn draft(&mut self, id: Uuid, lock: bool) -> Result<Draft> {
		Ok(AgentDraft::read(&mut self.tx, id, lock).await?.into())
	}
	async fn authorize_draft(&mut self, draft: &Draft, action: &str, shares: bool) -> Result<()> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::authorize(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &self.actor, policy),
			draft,
			action,
			shares,
		)
		.await
	}
	async fn limits(&mut self, tenant: &str) -> Result<TestLimits> {
		Ok(AgentTestLimit::locked(&mut self.tx, tenant).await?.into())
	}
	async fn save_limits(&mut self, limits: &TestLimits) -> Result<()> {
		AgentTestLimit::save(&mut self.tx, &limits.clone().into())
			.await
			.map_err(Into::into)
	}
	async fn session(&mut self, id: Uuid, lock: bool) -> Result<TestSession> {
		AgentTestSession::read(&mut self.tx, id, lock)
			.await
			.map_err(Into::into)
	}
	async fn sessions(&mut self, draft: Uuid) -> Result<Vec<TestSession>> {
		AgentTestSession::page(&mut self.tx, draft)
			.await
			.map_err(Into::into)
	}
	async fn stop(&mut self, id: Uuid) -> Result<TestSession> {
		AgentTestSession::stop(&mut self.tx, id)
			.await
			.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Box::new(self.tx)
			.commit()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}

mod dispatch;

mod execution;

mod admission;
