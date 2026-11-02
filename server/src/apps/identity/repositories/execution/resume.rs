//! Native worker scopes discard failed refresh transactions before releasing their mutex.
use crate::{
	apps::identity::services::{
		access::Access,
		execution::{authorize_guard, refresh_access_for_run},
	},
	domain::{Run, RunMetadata},
	federation::Federation,
	registry::AgentConfig,
};
use aidash_application::{
	Error, Result,
	ports::authorization::resume::{WorkerResumeRepository, WorkerResumeScope},
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
pub(crate) struct Resume<'a> {
	pub(crate) scratch_space_id: uuid::Uuid,
	pub(crate) federation: &'a Federation,
	pub(crate) remote: Option<&'a Federation>,
	pub(crate) access: &'a Arc<RwLock<Access>>,
	pub(crate) run: &'a Run,
	pub(crate) agent: &'a AgentConfig,
}
struct Scope<'a> {
	scratch_space_id: uuid::Uuid,
	federation: &'a Federation,
	access: OwnedRwLockWriteGuard<Access>,
	run: &'a Run,
	agent: &'a AgentConfig,
}
#[async_trait]
impl WorkerResumeRepository for Resume<'_> {
	fn remote(&self) -> bool {
		self.remote.is_some()
	}
	async fn refresh_remote(&self) -> Result<()> {
		aidash_application::authorization::tools::refresh(&crate::bootstrap::agent_tool_repository(
			self.remote,
			self.access,
			self.run,
		))
		.await
	}
	async fn lease(&self) -> Result<Box<dyn WorkerResumeScope + '_>> {
		Ok(Box::new(Scope {
			scratch_space_id: self.scratch_space_id,
			federation: self.federation,
			access: self.access.clone().write_owned().await,
			run: self.run,
			agent: self.agent,
		}))
	}
	async fn wait(&self, delay: Duration) {
		tokio::time::sleep(delay).await;
	}
}
#[async_trait]
impl WorkerResumeScope for Scope<'_> {
	async fn refresh(&mut self) -> Result<()> {
		refresh_access_for_run(&mut self.access, &self.federation.store, self.run)
			.await
			.map_err(Into::into)
	}
	async fn guard(&mut self) -> Result<()> {
		authorize_guard(
			self.federation,
			&RunMetadata::from(self.run),
			&mut self.access,
			true,
		)
		.await?;
		let mut services = crate::apps::workspaces::repositories::scratch::NativeScratchServices {
			policy: &self.federation.config.scratch_policy,
		};
		let mut adapter = crate::apps::workspaces::repositories::scratch::ScratchAdapter {
			access: &mut self.access,
			services: &mut services,
			person_override: None,
		};
		let current = aidash_application::scratch::scratch_space_for_run(
			&mut adapter,
			self.run,
			chrono::Utc::now(),
		)
		.await
		.map_err(aidash_application::authorization::worker_entry::scratch_refusal)?;
		if current != self.scratch_space_id {
			return Err(
				aidash_application::authorization::worker_entry::scratch_refusal(Error::Forbidden),
			);
		}
		Ok(())
	}
	async fn inference(&mut self) -> Result<()> {
		aidash_application::authorization::inference::authorize(
			&mut crate::bootstrap::inference_approval_scope(&mut self.access, self.run, false),
			&self.run.metadata(),
			self.agent,
		)
		.await
	}
	fn retryable(&self, error: &Error) -> bool {
		matches!(error, Error::TransactionPending)
			|| matches!(error,Error::Port(error) if error.downcast_ref::<crate::Error>().is_some_and(crate::Error::is_transient_database))
	}
	async fn discard_failed_refresh(&mut self) {
		self.access.discard_failed_execution_refresh().await;
	}
}
