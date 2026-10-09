//! Durable operations required by the agent use case. Implementations retain
//! lease/revision fences, transaction-scoped authority, audit, and outbox writes.
use crate::{
	Result,
	ports::{CompactionClassifier, ModelProvider},
};
use aidash_domain::{
	media::Selection,
	model::ModelConfig,
	provider::{ContentPart, ModelRequest, ModelResponse, ToolCall, ToolSpec},
	registry::{EntityRef, Entry},
	semantic::InputRead,
	*,
};
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

/// Validated projection of the registered agent configuration used by execution.
pub struct ExecutionAgent {
	pub model: EntityRef,
	pub instructions: String,
	pub knowledge_digest: Option<String>,
	pub tools: Vec<EntityRef>,
	pub skills: Vec<EntityRef>,
	pub max_steps: i32,
	pub allow_task_creation: Option<bool>,
	pub conversation_memory: bool,
	/// Pinned through the Run's Binding snapshot (ADR 0015).
	pub projection: aidash_domain::context::projection::ProjectionVersion,
}
pub struct InvocationOutcome {
	pub status: String,
	pub result: Option<Value>,
}
pub struct HumanMediaBatch {
	pub parts: Vec<ContentPart>,
	pub through_seq: Option<i64>,
	pub has_more: bool,
}
/// One `semantic_memory` retrieval for an inference.
#[derive(Debug, Clone, Default)]
pub struct SemanticRetrieval {
	/// The model-visible value; `None` sends no `semantic_memory`.
	pub value: Option<Value>,
	/// Opaque revisions `value` depends on beyond the ones it carries itself,
	/// captured only for `ProjectionVersion::Ordered`. `None` when the source
	/// cannot report them, so the value is never reused at a later step.
	pub dependencies: Option<Value>,
}
pub type Tools = BTreeMap<String, Arc<dyn ExecutionTool>>;
pub type ObservationFit<'a> = dyn Fn(usize, &Value) -> Result<bool> + Send + Sync + 'a;

#[async_trait]
pub trait ExecutionStore: Send + Sync {
	async fn save_run(&self, run: &Run, token: Uuid, event: &str) -> Result<()>;
	/// Persist inference inputs without ending the worker's current lease. Return
	/// the updated Run revision so the response uses the same durable boundary.
	async fn observe_sources(&self, run: &mut Run, token: Uuid) -> Result<()> {
		self.save_run(run, token, "run.sources_observed").await
	}
	async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event>;
	async fn run_inputs(&self, run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>>;
	async fn begin_final_completion(&self, run: &Run, token: Uuid) -> Result<bool>;
	async fn human_request(
		&self,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest>;
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest>;
	async fn expire_workbench_approval(&self, id: Uuid) -> Result<HumanRequest>;
	async fn invocation_start(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		name: &str,
		input: &Value,
		replay_safe: bool,
	) -> Result<InvocationOutcome>;
	async fn invocation_finish(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		output: &Value,
	) -> Result<()>;
	async fn reconciliation_request(
		&self,
		run: &mut Run,
		token: Uuid,
		key: &str,
		prompt: &str,
	) -> Result<()>;
	async fn run_message_has_media(&self, messages: &[Uuid]) -> Result<bool>;
}
#[async_trait]
pub trait ExecutionCatalog: Send + Sync {
	async fn get_for_run(&self, run: &Run, id: &str, version: &str) -> Result<Entry>;
	fn skill_instructions(&self, entry: &Entry) -> Result<String>;
	fn content_digest(&self, content: &str) -> String;
}
#[async_trait]
pub trait ExecutionHome: Send + Sync {
	async fn human_request(
		&self,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		let _ = (run, kind, prompt, key);
		Err(crate::Error::Invalid(
			"Home human-request route is unavailable".into(),
		))
	}
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		let _ = id;
		Err(crate::Error::Invalid(
			"Home human-request route is unavailable".into(),
		))
	}

	fn local(&self) -> bool;
	fn has_local_authority(&self) -> bool;
	async fn task(&self) -> Result<Task>;
	async fn claim(&self, task: &Task, agent: &Entry) -> Result<Task>;
	async fn transition(&self, next: TaskStatus) -> Result<Task>;
	async fn read_record(&self, kind: &str, id: &str) -> Result<Value>;
	async fn read_record_chunk(
		&self,
		kind: &str,
		id: &str,
		offset: usize,
		maximum: usize,
	) -> Result<Value>;
	async fn observation(&self, offset: usize, limit: usize) -> Result<Value>;
	async fn observation_fitted(
		&self,
		offset: usize,
		limit: usize,
		fits: &ObservationFit<'_>,
	) -> Result<Option<(usize, Value)>>;
	async fn child_summary(&self, parent: Uuid) -> Result<ChildTaskSummary>;
	async fn complete(&self, key: &str, artifact: &ArtifactInput) -> Result<Task>;
	async fn report(&self, key: &str, kind: &str, data: Value) -> Result<()>;
	async fn response_message(
		&self,
		token: Uuid,
		input_sequence: i64,
		key: &str,
		text: &str,
	) -> Result<()>;
}
#[async_trait]
pub trait ExecutionTool: Send + Sync {
	fn specification(&self) -> ToolSpec;
	fn contract(&self) -> aidash_domain::tool::ToolContract;
	fn replay_safe(&self) -> bool;
	async fn invoke(&self, run: &Run, input: Value, key: &str) -> Result<Value>;
}
#[async_trait]
pub trait InferenceReservation: Send {
	async fn settle(self: Box<Self>, response: &ModelResponse) -> Result<()>;
}
#[async_trait]
pub trait ExecutionAuthority: Send + Sync {
	fn is_remote(&self) -> bool;
	async fn action(&self, action: &str, kind: &str, id: Uuid) -> Result<()>;
	async fn inference(&self) -> Result<()>;
	async fn tool(
		&self,
		call: &ToolCall,
		contract: &aidash_domain::tool::ToolContract,
	) -> Result<()>;
	async fn human_read(&self, id: Uuid) -> Result<()>;
	async fn model_media(&self, selections: &[Selection]) -> Result<Vec<ContentPart>>;
	async fn human_message_media(
		&self,
		messages: &[(i64, Uuid, usize)],
		model: &ModelConfig,
	) -> Result<HumanMediaBatch>;
	async fn reserve_inference(
		&self,
		token: Uuid,
		window: usize,
		output: u32,
		request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>>;
	async fn suspend(&self) -> Result<()>;
	async fn resume(&self) -> Result<()>;
}
#[async_trait]
pub trait ExecutionVisibility: Send {
	async fn suspend(&mut self) -> Result<()>;
	async fn resume(&mut self) -> Result<()>;
}
/// Composition supplies concrete adapters without exposing their framework types.
/// A native authority owns its policy scope for the complete step, suspends only
/// around inference, and rechecks before provider output is accepted.
#[async_trait]
pub trait ExecutionEnvironment: Send + Sync {
	fn node_id(&self) -> &str;
	fn store(&self) -> &dyn ExecutionStore;
	fn catalog(&self) -> &dyn ExecutionCatalog;
	fn authority(&self) -> Option<&dyn ExecutionAuthority>;
	fn home(&self, run: &Run) -> Box<dyn ExecutionHome>;
	fn agent(&self, entry: &Entry) -> Result<ExecutionAgent>;
	fn provider(&self, model: ModelConfig) -> Result<Arc<dyn ModelProvider>>;
	fn compactor(&self) -> Result<Box<dyn CompactionClassifier>>;
	fn binding_resolver(&self) -> &dyn super::bindings::BindingResolver;
	async fn documents(&self, entry: &Entry) -> Result<Value>;
	async fn recheck_source_observation(&self, run: &Run, content: &Value) -> Result<()>;
	async fn skill_context(&self, run: &Run) -> Result<String>;
	/// The Tenant cache salt line for the Run's requests (ADR 0016), from
	/// `context::projection::cache_salt_line`. Fails when the node has no
	/// prompt cache key, because `Ordered` requests never go out unsalted.
	async fn prompt_cache_salt(&self, run: &Run) -> Result<String>;
	/// `projection` selects the model-visible envelope: `Ordered` omits the
	/// step and Run revision and reports `SemanticRetrieval::dependencies`.
	async fn semantic_context(
		&self,
		run: &Run,
		task: &Task,
		inputs: &[(InputRead, String)],
		budget: usize,
		entry: &Entry,
		projection: aidash_domain::context::projection::ProjectionVersion,
	) -> Result<SemanticRetrieval>;
	/// `ProjectionVersion::Ordered` only: whether `semantic`, retrieved at an
	/// earlier inference of this Run with `dependencies`, may be sent again.
	/// Re-authorizes its content and compares every dependency revision.
	/// `Ok(false)` means changed, narrowed or revoked: retrieve again.
	async fn semantic_observation_current(
		&self,
		run: &Run,
		semantic: &Value,
		dependencies: &Value,
	) -> Result<bool>;
	async fn run_message_limit(&self, run: &Run) -> Result<usize>;
	async fn run_request_headroom(&self, run: &Run) -> Result<usize>;
	async fn deliver_run_messages(&self, run: &Run) -> Result<()>;
	async fn reconcile_run_messages(&self, run: &Run) -> Result<()>;
	async fn require_terminal_safe_delivery(&self, run: &Run) -> Result<()>;
	async fn transition_terminal_run_messages(&self, run: &Run, status: TaskStatus) -> Result<()>;
	async fn wait_for_inference_cancellation(&self, run: Uuid) -> Result<()>;
	async fn operator_human_message_media(
		&self,
		run: &Run,
		messages: &[(i64, Uuid, usize)],
		model: &ModelConfig,
	) -> Result<HumanMediaBatch>;
}

pub mod media;

pub mod cancellation;

pub mod admission;

pub mod terminal;
pub mod worker;

pub mod headroom;
