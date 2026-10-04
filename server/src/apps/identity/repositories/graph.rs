//! All projection reads borrow the caller's existing policy and mapping locks.
use crate::{
	apps::identity::services::peer::graph::{self, GraphAuthority},
	federation::Federation,
};
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
