//! Current inference authority and running-only progress/finalization keep their original transactions.
use super::{Repository, Scope};
use crate::apps::registry::workbench::models::AgentTestSession;
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::execution::{ExecutionRepository, ExecutionScope},
};
use aidash_domain::registry::{
	Entry,
	workbench::{
		Draft,
		sandbox::{TestOutcome, TestSession},
	},
};
use async_trait::async_trait;

use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl ExecutionRepository for Repository {
	async fn begin_execution(&self) -> Result<Box<dyn ExecutionScope + '_>> {
		Ok(Box::new(Scope {
			tx: crate::database::native::begin(&self.store.pool).await?,
			actor: self.actor.clone(),
			node_id: self.node_id.clone(),
		}))
	}
	async fn read_session(&self, id: Uuid) -> Result<TestSession> {
		let lease = self.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| AgentTestSession::read(tx, id, false).await)
			.await
			.map_err(Into::into)
	}
	async fn progress(
		&self,
		id: Uuid,
		conversation: Value,
		calls: Value,
		usage: Value,
	) -> Result<()> {
		let lease = self.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| AgentTestSession::progress(tx, id, conversation, calls, usage).await)
			.await
			.map_err(Into::into)
	}
	async fn finish(&self, id: Uuid, outcome: &TestOutcome) -> Result<()> {
		let lease = self.store.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				AgentTestSession::finish(
					tx,
					id,
					&outcome.status,
					outcome.conversation.clone(),
					outcome.tool_calls.clone(),
					outcome.usage.clone(),
					outcome.error.clone(),
				)
				.await
			})
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl ExecutionScope for Scope {
	async fn bindings(
		&mut self,
		draft: &Draft,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let mut lookup = super::super::authority::Scope {
			policy: self.tx.pool().dashboard_policy(),
			tx: &mut *self.tx,
			actor: &self.actor,
		};
		let source = if draft
			.documents
			.as_array()
			.is_some_and(|docs| !docs.is_empty())
		{
			let mut raw: Entry = serde_json::from_value(draft.entry.clone())?;
			Some(aidash_application::registry::bindings::private::attach(
				&mut raw,
				&self.node_id,
				&draft.documents,
			)?)
		} else {
			None
		};
		let definitions: &mut dyn aidash_application::ports::registry::DefinitionLookup =
			if let Some(source) = &source {
				&mut aidash_application::registry::bindings::private::Preview {
					scope: &mut lookup,
					source,
				}
			} else {
				&mut lookup
			};
		aidash_application::registry::bindings::resolve(
			&mut aidash_application::registry::bindings::catalog::LookupCatalog {
				definitions,
				node: &self.node_id,
			},
			&crate::bootstrap::registry_validation(),
			aidash_domain::registry::bindings::QualifiedRef {
				registry_node: self.node_id.clone(),
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
			entry,
			false,
		)
		.await
	}

	async fn validate_content(&mut self, draft: &Draft) -> Result<Entry> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::validate_content(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &self.actor, policy),
			&crate::bootstrap::registry_validation(),
			draft,
			&self.node_id,
		)
		.await
	}
}
