//! Domain rules shared by HTTP, worker, and compaction paths.
pub use aidash_domain::context::{
	Context, ContextEvent, ContextUsage, MIN_CONTEXT_RESERVE, MessageReadCoverage, RequestBudget,
	agent_instructions, bound_snapshot, compaction_snapshot, estimated_tokens,
	request_context_budget, tool_event_growth,
};
pub(crate) mod observation;
