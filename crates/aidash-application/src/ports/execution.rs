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
	/// Opt-in Context Policy from the immutable Agent version.
	pub context_policy: Option<aidash_domain::context::policy::ContextPolicy>,
	/// Pinned through the Run's Binding snapshot; Legacy when the definition
	/// names none.
	pub projection_version: aidash_domain::projection::ProjectionVersion,
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
	/// Original Context Journal entries with `from <= seq <= through`, in order.
	async fn context_journal(
		&self,
		run: Uuid,
		from: u64,
		through: u64,
	) -> Result<Vec<aidash_domain::context::HistoryEntry>>;
	/// Journal every saved projection entry the Context Journal lacks, fenced
	/// by the lease and without saving the Run. A projection imported from a
	/// pre-journal Run is journaled whole here before any lossy compaction.
	async fn journal_context(&self, run: &Run, token: Uuid) -> Result<()>;
	/// Record a Compaction Attempt before provider I/O, fenced by the lease.
	/// Unsettled earlier attempts of the Run become `abandoned` in the same
	/// transaction. Returns `Error::Context(SummaryUnavailable)` once the Run
	/// already has `call_budget` attempts of the same stage.
	async fn begin_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: &aidash_domain::context::recovery::Attempt,
		call_budget: u32,
	) -> Result<()>;
	/// Settle an attempt that leaves the saved Context Projection unchanged.
	async fn settle_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: Uuid,
		settlement: &aidash_domain::context::recovery::Settlement,
	) -> Result<()>;
	/// Save `run`, whose context carries the adopted projection, and mark the
	/// attempt adopted in one lease-fenced transaction. Returns `Conflict` when
	/// the attempt is no longer open or the journal lacks its source range.
	async fn adopt_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: Uuid,
		settlement: &aidash_domain::context::recovery::Settlement,
	) -> Result<()>;
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
	/// Authorize one Summary Stage request for the exact pinned summarizer and
	/// charge its allowance before I/O: catalog approval, remote RequiredHome
	/// summarizer pin and disclosure, and every generated ancestor's summary
	/// allowance. Returns `Error::Context(SummaryUnavailable)` when any approval
	/// is missing; it never substitutes another provider.
	async fn reserve_summary(
		&self,
		token: Uuid,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
		window: usize,
		output: u32,
		request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>>;
	/// Repeat `reserve_summary`'s approval checks for the exact summarizer,
	/// without charging another call, after authority is resumed and before a
	/// Summary Stage candidate is adopted.
	async fn recheck_summary(
		&self,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
	) -> Result<()>;
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
	/// Whether every source an adopted Execution Summary depends on is still
	/// readable by the Run under current authority.
	async fn summary_dependencies_current(
		&self,
		run: &Run,
		dependencies: &aidash_domain::context::summary::SummaryDependencies,
	) -> Result<bool>;
}

pub mod media;

pub mod cancellation;

pub mod admission;

pub mod summary;

pub mod terminal;
pub mod worker;

pub mod headroom;
