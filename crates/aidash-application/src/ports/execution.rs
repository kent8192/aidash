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
	/// The Run Tool Parallelism ceiling pinned with the Binding snapshot.
	pub tool_parallelism: usize,
	/// Pinned through the Run's Binding snapshot; Legacy when the definition
	/// names none.
	pub projection_version: aidash_domain::projection::ProjectionVersion,
	/// Pinned through the Run's Binding snapshot; `off` when the definition
	/// names none (ADR 0019).
	pub prompt_cache: aidash_domain::projection::PromptCache,
}
pub struct InvocationOutcome {
	pub status: String,
	pub result: Option<Value>,
}
/// One call journaled by Tool Batch admission.
pub struct BatchInvocation<'a> {
	pub key: &'a str,
	pub name: &'a str,
	pub input: &'a Value,
}
/// The process's Tool Parallelism ceiling, shared by every worker in the process.
/// Each executing call of any Run holds one permit.
#[derive(Clone)]
pub struct ToolSlots {
	ceiling: usize,
	permits: Arc<tokio::sync::Semaphore>,
}
impl ToolSlots {
	pub fn new(ceiling: usize) -> Self {
		let ceiling = ceiling.max(1);
		Self {
			ceiling,
			permits: Arc::new(tokio::sync::Semaphore::new(ceiling)),
		}
	}
	pub fn ceiling(&self) -> usize {
		self.ceiling
	}
	/// Wait in FIFO order. A call never holds a permit while waiting for another.
	pub async fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
		self.permits
			.clone()
			.acquire_owned()
			.await
			.map_err(|_| crate::Error::Conflict("tool slots closed".into()))
	}
}
pub struct HumanMediaBatch {
	pub parts: Vec<ContentPart>,
	pub through_seq: Option<i64>,
	pub has_more: bool,
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
	/// Admit a Tool Batch atomically: persist the Run (whose ToolCall state
	/// records the batch end) and journal every call as STARTED under the
	/// worker lease and the stale-input check, before any call is dispatched.
	/// Batched calls are replay-safe; existing rows are returned unchanged.
	async fn invocation_start_batch(
		&self,
		run: &Run,
		token: Uuid,
		calls: &[BatchInvocation<'_>],
	) -> Result<Vec<InvocationOutcome>>;
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
	/// Resource Claims and output bound derived by trusted provider code from
	/// validated arguments. `None` keeps the call on the sequential path.
	fn concurrent_call(
		&self,
		_input: &Value,
	) -> Option<aidash_domain::tool::concurrency::ConcurrentCall> {
		None
	}
	/// Dispatch-time checks that `invoke` performs before execution. A Tool Batch
	/// admits every call before dispatching any of them, while it holds the
	/// Run's authority scope, so checks never wait behind a sibling's execution.
	async fn admit(&self, _run: &Run, input: Value) -> Result<Value> {
		Ok(input)
	}
	/// Execute an input returned by `admit`.
	async fn dispatch(&self, run: &Run, admitted: Value, key: &str) -> Result<Value> {
		self.invoke(run, admitted, key).await
	}
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
	/// The process's Tool Parallelism ceiling. `None` keeps every call sequential.
	fn tool_slots(&self) -> Option<&ToolSlots> {
		None
	}
	fn provider(&self, model: ModelConfig) -> Result<Arc<dyn ModelProvider>>;
	fn compactor(&self) -> Result<Box<dyn CompactionClassifier>>;
	fn binding_resolver(&self) -> &dyn super::bindings::BindingResolver;
	async fn documents(&self, entry: &Entry) -> Result<Value>;
	async fn recheck_source_observation(&self, run: &Run, content: &Value) -> Result<()>;
	async fn skill_context(&self, run: &Run) -> Result<String>;
	/// Current revision of the Run's pinned Skill record, `None` before one
	/// exists. An Ordered Run reuses its Skill context while this is unchanged.
	async fn skill_revision(&self, run: &Run) -> Result<Option<i64>>;
	/// Cache Scope for a salted Projection Version: the Tenant whose provider
	/// cache the request may share and the current Cache Salt Key version. Fails
	/// with a typed error when the node has no Cache Salt Key; never unsalted.
	async fn cache_scope(&self, run: &Run) -> Result<aidash_domain::projection::CacheScope>;
	/// Authority scope and source revisions of an Ordered Retrieval Key. Cheap
	/// reads only; a remote Run reports its admission values.
	async fn retrieval_scope(
		&self,
		run: &Run,
	) -> Result<aidash_domain::context::sources::RetrievalScope>;
	/// `key` is present for an Ordered Run: equal keys replay the same bytes.
	async fn semantic_context(
		&self,
		run: &Run,
		task: &Task,
		inputs: &[(InputRead, String)],
		budget: usize,
		entry: &Entry,
		key: Option<&aidash_domain::context::sources::RetrievalKey>,
	) -> Result<Option<Value>>;
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
