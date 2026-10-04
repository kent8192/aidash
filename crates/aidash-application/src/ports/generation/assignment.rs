//! Admission ports retain the caller's authority, policy lock and atomic reservation.
use crate::Result;
use aidash_domain::{
	Task,
	federation::Delegation,
	generation::{
		policy::Policy,
		requests::{Assignment, Request},
	},
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry, Search},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

pub struct Creation<'a> {
	pub id: Uuid,
	pub task: &'a Task,
	pub policy: &'a Policy,
	pub definition: &'a Entry,
	pub status: &'a str,
	pub reason: &'a str,
	pub depth: i32,
	pub lifetime_seconds: f64,
	pub home_node: &'a str,
	pub foreign_intent: Option<Value>,
}
#[async_trait]
pub trait GenerationCreationScope: Send {
	fn tenant(&self) -> &str;
	fn subject(&self) -> &str;
	fn subjects(&self) -> &[String];
	fn bundle(&self) -> &PolicyBundle;
	fn node_id(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	fn request_id(&self) -> Uuid;
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn previous_depth(&mut self) -> Result<Option<i32>>;
	async fn active(&mut self, policy_id: &str) -> Result<i64>;
	async fn insert(&mut self, creation: &Creation<'_>) -> Result<Request>;
	async fn allocate(
		&mut self,
		policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> Result<()>;
	async fn budget(
		&mut self,
		id: Uuid,
		policy: &Policy,
		compaction_calls: i64,
		embedding_calls: i64,
	) -> Result<()>;
	async fn history(&mut self, id: Uuid, status: &str, reason: &str) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
	async fn visible(&mut self, job: &Request) -> Result<bool>;
}
#[async_trait]
pub trait GenerationAssignmentScope: GenerationCreationScope {
	fn context(&mut self, value: Value);
	fn replace_subjects(&mut self, subjects: Vec<String>);
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn task(&mut self, id: Uuid) -> Result<Task>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<bool>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn policy(&mut self, id: &str) -> Result<Policy>;
	async fn existing(&mut self, task_id: Uuid) -> Result<Option<Request>>;
	async fn existing_run(
		&mut self,
		task_id: Uuid,
	) -> Result<Option<(String, String, Vec<String>)>>;
	async fn catalog(&mut self, search: &Search) -> Result<Vec<Entry>>;
	async fn generated(&mut self, entry: &Entry) -> Result<bool>;
	async fn delegate(&mut self, task_id: Uuid, agent: &EntityRef) -> Result<Delegation>;
}
#[async_trait]
pub trait GenerationAssignmentSession: GenerationAssignmentScope {
	async fn finish(self: Box<Self>, result: Result<Assignment>) -> Result<Assignment>;
}
#[async_trait]
pub trait GenerationAssignments: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn GenerationAssignmentSession>>;
	fn notify(&self);
}
