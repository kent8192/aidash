//! Source authority scopes retain policy, credential, grant and task locks until audited completion.
use crate::Result;
use aidash_domain::{
	NewTask, Task,
	federation::execution::{
		Description, PrepareInput, Prepared,
		admission::Admission,
		home::{Grant, HomeBinding},
	},
	identity::execution::ExecutionPrincipal,
	policy::Resource,
	semantic::{
		Failure,
		remote::{
			Binding,
			status::{Provenance, Status},
		},
	},
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait HomeScope: Send + Sized {
	fn identity(&self) -> ExecutionPrincipal;
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource;
	async fn task_read(&mut self, id: Uuid) -> Result<Task>;
	/// The current control projection uses the original task SHARE lock, without reading producer text.
	async fn control_task(&mut self, id: Uuid) -> Result<Option<Task>>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn requester_grant(&mut self, task: Uuid, grant: Uuid) -> Result<Option<Grant>>;
	async fn binding(&mut self, grant: Uuid) -> Result<Option<HomeBinding>>;
	async fn grants(&mut self, task: Uuid) -> Result<Vec<Grant>>;
	async fn delegation_grants(&mut self, task: &Task, node: &str) -> Result<Vec<Grant>>;
	async fn insert_binding(
		&mut self,
		grant: Uuid,
		task: Uuid,
		admission: &Admission,
		description: &Description,
	) -> Result<()>;
	async fn revoke(&mut self, grant: Uuid) -> Result<()>;
	async fn delivered_inputs(&mut self, task: Uuid, admission: Uuid) -> Result<Vec<String>>;
	async fn cancel_task(
		&mut self,
		task: Uuid,
		revision: i64,
		owner: &str,
		admission: Uuid,
		keys: &[String],
	) -> Result<Task>;
	async fn binding_revision(&mut self, grant: Uuid, revision: i64) -> Result<()>;
	async fn resume_semantic(
		&mut self,
		grant: Uuid,
		admission: Uuid,
		workspace: Uuid,
		subject: &str,
	) -> Result<()>;
	async fn remote_semantic_sources(&mut self, grant: Uuid) -> Result<()>;
	async fn grant_output_visible(&mut self, grant: Uuid) -> Result<bool>;
	async fn receipt(&mut self, grant: Uuid) -> Result<Option<Value>>;
	async fn provenance(&mut self, value: Option<Value>) -> Result<Option<Provenance>>;
	async fn create_task(
		&mut self,
		workspace: Uuid,
		input: &NewTask,
		subject: &str,
		key: &str,
	) -> Result<Task>;
	/// Rejected effects retain durable denial audit behavior; cancellation drops the physical transaction.
	async fn finish(self, result: Result<()>) -> Result<()>;
}
#[async_trait]
pub trait HomeRepository: Send + Sync {
	type Scope: HomeScope;
	fn node_id(&self) -> &str;
	fn identity(&self) -> Option<ExecutionPrincipal>;
	async fn begin(&self) -> Result<Self::Scope>;
	async fn description(&self, node: &str, grant: Uuid) -> Result<(Self::Scope, Description)>;
	/// Deserialize directly from bounded response bytes to preserve strict wire decoding.
	async fn request<T: DeserializeOwned + Send>(
		&self,
		node: &str,
		path: &str,
		input: &Value,
	) -> Result<T>;
	async fn prepare(&self, task: Uuid, input: PrepareInput) -> Result<Prepared>;
	async fn semantic_status(
		&self,
		grant: Uuid,
		binding: &Binding,
		reason: Option<Failure>,
	) -> Result<Status>;
}
