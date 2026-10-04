//! Graph discovery uses one caller-owned authority transaction throughout a page.
use crate::Result;
use aidash_domain::federation::graph::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;
#[async_trait]
pub trait GraphProjectionScope: Send {
	fn node_id(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	fn encode_cursor(&self, cursor: &GraphCursor) -> Result<String>;
	/// Authenticate and decode the opaque cursor. The application checks its scope and lifetime.
	fn decode_cursor(&self, token: &str) -> Result<GraphCursor>;
	async fn generation(&mut self, options: &GraphOptions) -> Result<String>;
	async fn candidates(
		&mut self,
		kind: u8,
		offset: u64,
		options: &GraphOptions,
	) -> Result<Vec<Candidate>>;
	async fn visible(&mut self, candidate: &Candidate, scoped: bool) -> Result<bool>;
	async fn linked_workspace(&mut self, id: Uuid) -> Result<Option<Candidate>>;
	async fn linked_task(&mut self, id: Uuid) -> Result<Option<Candidate>>;
	async fn linked_registry(
		&mut self,
		kind: &str,
		id: &str,
		version: &str,
	) -> Result<Option<Candidate>>;
	async fn activity(
		&mut self,
		options: &GraphOptions,
		nodes: &[GraphNode],
		window_end: i64,
	) -> Result<Vec<GraphActivity>>;
}

/// Candidate routing uses the current viewer's caller-owned authority scope.
#[async_trait]
pub trait GraphVisibility: Send {
	fn operator(&self) -> bool;
	async fn operator_workspace(&mut self, id: Uuid) -> Result<bool>;
	async fn registry(
		&mut self,
		entry: &aidash_domain::registry::Entry,
		action: &str,
	) -> Result<bool>;
	async fn workspace(&mut self, id: Uuid, action: &str) -> Result<bool>;
	async fn record(&mut self, candidate: &Candidate) -> Result<bool>;
	async fn conversation(
		&mut self,
		conversation: &aidash_domain::Conversation,
		action: &str,
	) -> Result<bool>;
}
