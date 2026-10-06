//! Scoped read policies borrow the current authority, dependency frontier, and transaction.
use crate::Result;
use aidash_domain::{
	Conversation, Event, HumanRequest, RunMetadata, Task, generation::requests::Request,
	policy::Resource,
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait LocalRunVisibilityScope: Send {
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
pub trait EventVisibilityScope: Send {
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
