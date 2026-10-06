//! Current worker authority retains the inherited source scope through dependency commit.
use crate::{
	Error, Result as NativeResult,
	apps::identity::services::access::Access,
	domain::{Run, Task},
	federation::Federation,
	registry::AgentConfig,
	store::Store,
};
use aidash_application::{
	Result,
	ports::semantic::run_context::{RunSemanticJournal, RunSemanticRepository, RunSemanticScope},
};
use aidash_domain::semantic::{InputRead, results::SearchResult};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};
pub(crate) struct ContextRepository<'a> {
	pub(crate) store: &'a Store,
	pub(crate) remote: Option<&'a Federation>,
	pub(crate) access: &'a Arc<Mutex<Access>>,
	pub(crate) run: &'a Run,
	pub(crate) agent: &'a AgentConfig,
}
struct Scope<'a> {
	store: &'a Store,
	access: OwnedMutexGuard<Access>,
	run: &'a Run,
	agent: &'a AgentConfig,
}
#[async_trait]
impl RunSemanticRepository for ContextRepository<'_> {
	fn remote(&self) -> bool {
		self.remote.is_some()
	}
	async fn suspend(&self) -> Result<()> {
		let result: NativeResult<()> = async {
			let mut access = self.access.lock().await;
			if access.tx.is_active() {
				access.suspend().await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn remote_context(
		&self,
		task: &Task,
		inputs: &[(InputRead, String)],
		query: &str,
		budget: usize,
	) -> Result<Option<Value>> {
		let federation = self.remote.ok_or(Error::Forbidden)?;
		crate::apps::identity::services::peer::semantic::context(
			federation, self.run, task, inputs, query, budget,
		)
		.await
		.map_err(Into::into)
	}
	async fn refresh_remote(&self) -> Result<()> {
		aidash_application::authorization::tools::refresh(&crate::bootstrap::agent_tool_repository(
			self.remote,
			self.access,
			self.run,
		))
		.await
	}
	async fn local_scope(&self) -> Result<Box<dyn RunSemanticScope + '_>> {
		Ok(Box::new(Scope {
			store: self.store,
			access: self.access.clone().lock_owned().await,
			run: self.run,
			agent: self.agent,
		}))
	}
}
#[async_trait]
impl RunSemanticScope for Scope<'_> {
	async fn retrieve(&mut self, query: &str, budget: usize) -> Result<Option<SearchResult>> {
		crate::semantic::service::context_in(
			self.store,
			&mut crate::semantic::service::Lease::Inherited(&mut self.access),
			self.run,
			query,
			budget,
			self.agent,
		)
		.await
		.map_err(Into::into)
	}
	async fn begin_dependencies(&mut self) -> Result<Box<dyn RunSemanticJournal + '_>> {
		Ok(Box::new(
			crate::apps::knowledge::repositories::run_context::Journal::begin(
				self.store,
				self.run.id,
			)
			.await?,
		))
	}
}
