//! All projection reads borrow the caller's existing policy and mapping locks.
use self::persistence::{self as graph, GraphAuthority};
use crate::federation::Federation;
use aidash_application::{Result, ports::graph::GraphProjectionScope};
use aidash_domain::federation::graph::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;
pub(crate) struct Projection<'a, 'scope> {
	pub f: &'a Federation,
	pub authority: &'a mut GraphAuthority<'scope>,
	pub source_node: &'a str,
	pub revision: &'a str,
}
#[async_trait]
impl GraphProjectionScope for Projection<'_, '_> {
	fn node_id(&self) -> &str {
		&self.f.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	fn encode_cursor(&self, cursor: &GraphCursor) -> Result<String> {
		graph::encode_cursor(self.f, cursor).map_err(Into::into)
	}
	fn decode_cursor(&self, token: &str) -> Result<GraphCursor> {
		graph::decode_cursor(self.f, token).map_err(Into::into)
	}
	async fn generation(&mut self, options: &GraphOptions) -> Result<String> {
		graph::graph_generation(
			self.f,
			self.authority,
			self.revision,
			options,
			self.source_node,
		)
		.await
		.map_err(Into::into)
	}
	async fn candidates(
		&mut self,
		kind: u8,
		offset: u64,
		options: &GraphOptions,
	) -> Result<Vec<Candidate>> {
		graph::candidates(self.authority, kind, offset, options, self.source_node)
			.await
			.map_err(Into::into)
	}
	async fn visible(&mut self, candidate: &Candidate, scoped: bool) -> Result<bool> {
		graph::candidate_visible(self.f, self.authority, candidate, scoped)
			.await
			.map_err(Into::into)
	}
	async fn linked_workspace(&mut self, id: Uuid) -> Result<Option<Candidate>> {
		graph::linked_workspace(self.authority, id)
			.await
			.map_err(Into::into)
	}
	async fn linked_task(&mut self, id: Uuid) -> Result<Option<Candidate>> {
		graph::linked_task(self.authority, id)
			.await
			.map_err(Into::into)
	}
	async fn linked_registry(
		&mut self,
		kind: &str,
		id: &str,
		version: &str,
	) -> Result<Option<Candidate>> {
		graph::linked_registry(self.authority, kind, id, version)
			.await
			.map_err(Into::into)
	}
	async fn activity(
		&mut self,
		options: &GraphOptions,
		nodes: &[GraphNode],
		window_end: i64,
	) -> Result<Vec<GraphActivity>> {
		graph::project_activity(
			self.f,
			self.authority,
			options,
			nodes,
			window_end,
			self.source_node,
		)
		.await
		.map_err(Into::into)
	}
}

pub(crate) mod persistence;

pub(crate) struct Visibility<'a, 'scope>(pub(crate) &'a mut GraphAuthority<'scope>);
#[async_trait]
impl aidash_application::ports::graph::GraphVisibility for Visibility<'_, '_> {
	fn operator(&self) -> bool {
		matches!(self.0, GraphAuthority::Operator { .. })
	}
	async fn operator_workspace(&mut self, id: Uuid) -> Result<bool> {
		crate::authorization::remote::operator::visible(self.0.connection(), id)
			.await
			.map_err(Into::into)
	}
	async fn registry(
		&mut self,
		entry: &aidash_domain::registry::Entry,
		action: &str,
	) -> Result<bool> {
		let GraphAuthority::Subject(access) = &mut self.0 else {
			return Err(aidash_application::Error::Forbidden);
		};
		let resource = crate::authorization::catalog::resource(access, entry);
		access.decide(&resource, action).await.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid, action: &str) -> Result<bool> {
		let GraphAuthority::Subject(access) = &mut self.0 else {
			return Err(aidash_application::Error::Forbidden);
		};
		access.allowed(id, action).await.map_err(Into::into)
	}
	async fn record(&mut self, candidate: &Candidate) -> Result<bool> {
		let GraphAuthority::Subject(access) = &mut self.0 else {
			return Err(aidash_application::Error::Forbidden);
		};
		match candidate {
			Candidate::Task(row) => access.task_visible(row).await.map_err(Into::into),
			Candidate::Run(row) => access.run_visible(row).await.map_err(Into::into),
			Candidate::Artifact(row) => access.artifact_visible(row).await.map_err(Into::into),
			_ => Err(aidash_application::Error::Invalid(
				"record visibility requires a task, run or artifact".into(),
			)),
		}
	}
	async fn conversation(
		&mut self,
		conversation: &aidash_domain::Conversation,
		action: &str,
	) -> Result<bool> {
		let GraphAuthority::Subject(access) = &mut self.0 else {
			return Err(aidash_application::Error::Forbidden);
		};
		let resource = access.conversation_resource(conversation).await?;
		access.decide(&resource, action).await.map_err(Into::into)
	}
}
