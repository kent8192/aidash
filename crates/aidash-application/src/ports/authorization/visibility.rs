//! Scoped read policies borrow the current authority, dependency frontier, and transaction.
use crate::Result;
use aidash_domain::{
	Conversation, Event, HumanRequest, RunMetadata, Task, generation::requests::Request,
	policy::Resource,
};
use async_trait::async_trait;
use std::collections::HashMap;
use uuid::Uuid;

/// Run verdicts shared by the events of one scan. A scan reads under one
/// authority, so every event of a Run takes that Run's verdict; the next scan
/// starts empty and applies live human membership and read provenance again.
#[derive(Default)]
pub struct RunVerdicts(HashMap<(Uuid, Option<Uuid>), bool>);

impl RunVerdicts {
	pub fn get(&self, run: Uuid, workspace: Option<Uuid>) -> Option<bool> {
		self.0.get(&(run, workspace)).copied()
	}
	pub fn remember(&mut self, run: Uuid, workspace: Option<Uuid>, visible: bool) {
		self.0.insert((run, workspace), visible);
	}
	#[cfg(test)]
	pub(crate) fn remembered(&self) -> usize {
		self.0.len()
	}
}

/// What a list loop learns about one row beyond visible or not: whether its
/// check gave up on a Node that did not answer (#770). A loop starts the row,
/// runs the check and takes the Node when the row is not visible, so a refusal
/// is never counted. A scope that reaches no other Node keeps the defaults.
pub trait RowOutcome {
	/// A row's check begins: forget the Node an earlier row gave up on.
	fn begin_row(&mut self) {}
	/// The Node this row's check gave up on so far.
	fn row_silence(&self) -> Option<String> {
		None
	}
	/// The Node this row's check gave up on, after the check.
	fn take_row_silence(&mut self) -> Option<String> {
		None
	}
	/// Put back (or clear) the Node the row's check gave up on, once the rest of
	/// a chain has been judged.
	fn set_row_silence(&mut self, _node: Option<String>) {}
}

/// A check that can meet a silent Node was followed by others in a chain, and
/// stopped the chain. `held` is the Node it stopped at, taken from the scope
/// before the rest of the chain ran, and `rest` whether the rest passed. The
/// row counts as unconfirmed only if nothing else refuses it: when the rest
/// passed, or stopped at a silent Node itself, the first Node is put back;
/// when the rest was refused, the row is a refusal and nothing is kept (#770).
pub fn conclude_chain(scope: &mut (impl RowOutcome + ?Sized), held: Option<String>, rest: bool) {
	if let Some(node) = held
		&& (rest || scope.row_silence().is_some())
	{
		scope.set_row_silence(Some(node));
	}
}

#[async_trait]
pub trait LocalRunVisibilityScope: Send + RowOutcome {
	fn cached(&self, run: &RunMetadata) -> Option<bool>;
	fn remember(&mut self, run: &RunMetadata, allowed: bool);
	fn resource(&self, kind: &str, id: &str, attributes: serde_json::Value) -> Resource;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn memory_resource(
		&mut self,
		run: &RunMetadata,
		workspace: &Resource,
	) -> Result<Resource>;
	async fn task(&mut self, run: &RunMetadata) -> Result<Option<Task>>;
	async fn task_visible(&mut self, task: &Task) -> Result<bool>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn human_reads(&mut self, workspace: Uuid, run: Uuid) -> Result<bool>;
	async fn core_area_visible(&mut self, run: Uuid) -> Result<bool>;
	async fn run_reads(&mut self, run: Uuid) -> Result<bool>;
}

#[async_trait]
pub trait RunVisibilityScope: LocalRunVisibilityScope {
	fn node_id(&self) -> &str;
	async fn scoped_admission(&mut self, run: Uuid) -> Result<bool>;
	async fn foreign_base_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	async fn legacy_execution(&mut self, run: &RunMetadata) -> Result<bool>;
}

#[async_trait]
pub trait EventVisibilityScope: Send + RowOutcome {
	async fn marketplace_visible(&mut self, event: &Event) -> Result<bool>;
	async fn resource_visible(&mut self, event: &Event) -> Result<Option<bool>>;
	async fn generation(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Request>>;
	async fn generation_visible(&mut self, job: &Request) -> Result<bool>;
	async fn conversation(
		&mut self,
		id: Uuid,
		workspace: Option<Uuid>,
	) -> Result<Option<Conversation>>;
	async fn conversation_resource(&mut self, conversation: &Conversation) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn human(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<HumanRequest>>;
	async fn human_visible(&mut self, request: &HumanRequest) -> Result<bool>;
	async fn run(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<RunMetadata>>;
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
}

pub mod resources;

pub mod provenance;
