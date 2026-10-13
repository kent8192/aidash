//! Typed recovery for a Run stopped by its semantic context (Issue #127).
//!
//! The cause is the exact [`Failure`]; the actions are the only next steps the
//! current contract authorizes. None of them continues a Run from context that
//! a source change invalidated, and none copies that context, its approvals or
//! its verification into new work.
use super::Failure;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "SemanticRecoveryAction")]
pub enum Action {
	/// Indexing or the semantic backend is still working and the Run holds an
	/// automatic retry; it retries on its own schedule and nobody needs to act.
	Wait,
	/// An authorized manual retry resumes the Run with its unchanged Binding. The
	/// retry passes only if every pinned revision and current authority hold.
	RetryUnchangedBinding,
	/// Create a new Task whose instructions the requester supplies explicitly. It
	/// has no parent or dependency link to the stopped Run, so no invalidated
	/// context, approval or verification is carried over.
	CreateFollowUpTask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "SemanticRecovery")]
pub struct Recovery {
	pub cause: Failure,
	/// Authorized next steps, most preferred first. Never empty.
	pub actions: Vec<Action>,
}

impl Recovery {
	/// `retry_scheduled` is whether the Run already holds an automatic retry.
	/// Without one, as for a paused Run, nothing observes a pending operation
	/// complete, so waiting is never offered.
	pub fn new(cause: Failure, retry_scheduled: bool) -> Self {
		use Action::*;
		let actions = match cause {
			// A stale reuse retries automatically with a fresh retrieval, like an
			// outage; once exhausted it pauses as `RetriesExhausted`.
			Failure::Pending | Failure::Unavailable | Failure::Stale if retry_scheduled => {
				vec![Wait]
			}
			// Retrying the same request cannot change the outcome: the Run's
			// context already holds obsolete text, or the same budget overflows.
			Failure::Invalidated | Failure::ContextBudget => vec![CreateFollowUpTask],
			Failure::Pending
			| Failure::Unavailable
			| Failure::Stale
			| Failure::RetriesExhausted
			| Failure::Authority
			| Failure::Configuration
			| Failure::Allowance
			| Failure::ProviderContract => vec![RetryUnchangedBinding, CreateFollowUpTask],
		};
		Self { cause, actions }
	}
}

#[cfg(test)]
mod tests;
