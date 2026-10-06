//! Session admission reuses the native expiry, concurrency count and active-slot insert on one scope.
use super::{Repository, Scope};
use crate::apps::registry::workbench::models::AgentTestSession;
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::admission::{AdmissionRepository, AdmissionScope},
};
use aidash_domain::registry::workbench::{
	Draft,
	sandbox::{TestLimits, TestSession},
};
use async_trait::async_trait;

use serde_json::Value;
#[async_trait]
impl AdmissionRepository for Repository {
	async fn begin_admission(&self) -> Result<Box<dyn AdmissionScope + '_>> {
		Ok(Box::new(Scope {
			tx: crate::database::native::begin(&self.store.pool).await?,
			actor: self.actor.clone(),
			node_id: self.node_id.clone(),
		}))
	}
}
#[async_trait]
impl AdmissionScope for Scope {
	async fn admit(
		&mut self,
		draft: &Draft,
		limits: &TestLimits,
		scenario: Value,
		conversation: Value,
	) -> Result<TestSession> {
		AgentTestSession::admit(
			&mut self.tx,
			&draft.clone().into(),
			&limits.clone().into(),
			scenario,
			conversation,
		)
		.await
		.map_err(Into::into)
	}
}
