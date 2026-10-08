//! Typed effects run inside the same source authority transaction as command admission.
use super::RemoteCommandScope;
use crate::Result;
use aidash_domain::{
	ArtifactInput, Message, NewTask, Task, TaskStatus,
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct WriteContext<'a> {
	pub task: &'a Task,
	pub owner: &'a str,
	pub key: &'a str,
	pub admission: Uuid,
}
pub enum Effect<'a> {
	HumanRequest {
		kind: &'a str,
		prompt: &'a str,
	},
	Claim {
		revision: i64,
		agent: &'a Entry,
	},
	Transition {
		revision: i64,
		next: TaskStatus,
		through: Option<i64>,
	},
	Artifact {
		artifact: &'a ArtifactInput,
		complete_through: Option<i64>,
	},
	CreateTask {
		task: &'a NewTask,
	},
	Delegate {
		child: Uuid,
		agent: &'a EntityRef,
	},
	Message {
		content: &'a str,
		through: Option<i64>,
	},
	ReserveInput {
		node: &'a str,
		key: &'a str,
		content: &'a str,
	},
	CommitInput {
		key: &'a str,
		content: &'a str,
		seq: i64,
	},
	DeliverInput {
		node: &'a str,
		key: &'a str,
		content: &'a str,
	},
	ReleaseInputs {
		node: &'a str,
		keys: &'a [String],
	},
	Event {
		kind: &'a str,
		data: Value,
	},
}
pub struct Applied {
	pub value: Value,
	/// Identifier of the output whose future reads are scoped to this grant.
	pub output_id: Option<Uuid>,
}
#[async_trait]
pub trait RemoteCommandEffects: RemoteCommandScope {
	fn local_node(&self) -> &str;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn artifact_creation_resource(&mut self, task: Uuid, owner: &str) -> Result<Resource>;
	async fn snapshot(&mut self, workspace: Uuid) -> Result<Value>;
	async fn record(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Value>;
	async fn children(&mut self, workspace: Uuid, parent: Uuid) -> Result<Value>;
	async fn created_by_grant(&mut self, grant: Uuid, child: Uuid) -> Result<bool>;
	async fn apply(&mut self, context: WriteContext<'_>, effect: Effect<'_>) -> Result<Applied>;
	async fn record_output(
		&mut self,
		grant: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()>;
	async fn human_read(
		&mut self,
		grant: Uuid,
		admission: Uuid,
		id: Uuid,
	) -> Result<aidash_domain::HumanRequest>;
	async fn history(
		&mut self,
		workspace: Uuid,
		task: Uuid,
		node: &str,
		offset: usize,
	) -> Result<Vec<Message>>;
	async fn persist_replay(
		&mut self,
		grant: Uuid,
		task: Uuid,
		key: &str,
		digest: &str,
		result: &Value,
	) -> Result<()>;
	async fn live(&mut self, grant: Uuid) -> Result<bool>;
}
