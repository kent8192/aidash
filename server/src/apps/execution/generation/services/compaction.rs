//! Every provider attempt reserves a durable call against all generated ancestors.
//! The outer worker authority lease remains held through the HTTP request.
use crate::{
	Error, Result,
	authorization::access::Access,
	context::jev::{JevAsker, JevClient, Questions},
	domain::Run,
	registry::EntityRef,
	store::Store,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

pub(crate) struct ApprovedCompactor {
	pub access: Arc<RwLock<Access>>,
	pub store: Store,
	pub run: Run,
	pub client: reqwest::Client,
	pub remote: Option<crate::federation::Federation>,
}

#[async_trait::async_trait]
impl JevAsker for ApprovedCompactor {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value> {
		if let Some(f) = &self.remote {
			return self.ask_remote(f, state, questions).await.map_err(|error| {
				Error::RemoteSemantic(crate::authorization::remote::semantic::failure(&error))
			});
		}
		let mut access = self.access.write().await;
		let transport = aidash_application::generation::compaction::reserve(
			&mut crate::bootstrap::generation_compaction_authority_scope(&mut access),
			&crate::bootstrap::generation_compaction_repository(&self.store),
			&crate::bootstrap::generation_compaction_provider(),
			&aidash_domain::generation::compaction::Context {
				run: self.run.id,
				task: self.run.task_id,
				agent: EntityRef {
					id: self.run.agent_id.clone(),
					version: self.run.agent_version.clone(),
				},
			},
			state,
			questions,
		)
		.await?;
		// Release the mutex before network I/O; the process-owned Access retains
		// the outer authority lease through the provider attempt.
		drop(access);
		if let Some(transport) = transport {
			transport.ask(state, questions).await.map_err(Into::into)
		} else {
			JevClient::from_env(self.client.clone())?
				.ask(state, questions)
				.await
		}
	}
}

impl ApprovedCompactor {
	async fn ask_remote(
		&self,
		federation: &crate::federation::Federation,
		state: &Value,
		questions: &Questions,
	) -> Result<Value> {
		let mut access = self.access.write().await;
		aidash_application::generation::compaction::remote::ask(
			&mut crate::bootstrap::generation_remote_compaction_scope(&mut access, federation),
			&crate::bootstrap::generation_compaction_provider(),
			&self.run,
			state,
			questions,
		)
		.await
		.map_err(Into::into)
	}
}
