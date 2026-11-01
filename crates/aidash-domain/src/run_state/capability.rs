//! Persisted Run state that older workers cannot read is gated by capability.
//!
//! A worker declares the [`PersistedCapability`] values its build understands.
//! The state a Run persists computes which of them it needs, and `encode` stores
//! that requirement in the `pending` envelope beside the state so Run leasing can
//! filter without decoding. Decoding recomputes it as a consistency check.
use super::{RecoveryState, ResumeState, RunState, ToolCallState, WaitingState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// A persisted-state feature whose absence makes a worker unable to load the state.
///
/// To add one, follow `docs/operations/worker-activation.md`.
#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
	schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PersistedCapability {
	/// `ToolCallState` carries the end of an admitted Tool Batch.
	ToolBatchState,
	/// `RecoveryState` remembers steps that ran with Degraded context (ADR 0059).
	DegradedContext,
	/// A logical MCP call retains its continuation and input checkpoint.
	McpCallState,
	/// A sent MCP round keeps its identity while a live callback waits.
	McpSentRound,
	/// Authentication waits retain the unexecuted call and once-per-resource history.
	ConnectionWait,
}
impl PersistedCapability {
	pub const ALL: &'static [Self] = &[
		Self::ToolBatchState,
		Self::DegradedContext,
		Self::McpCallState,
		Self::McpSentRound,
		Self::ConnectionWait,
	];

	/// What this build can load. A worker only leases Runs whose state needs a subset.
	pub fn supported() -> &'static [Self] {
		Self::ALL
	}
	pub fn name(self) -> &'static str {
		match self {
			Self::ToolBatchState => "tool_batch_state",
			Self::DegradedContext => "degraded_context",
			Self::McpCallState => "mcp_call_state",
			Self::McpSentRound => "mcp_sent_round",
			Self::ConnectionWait => "connection_wait",
		}
	}
}
impl std::fmt::Display for PersistedCapability {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.name())
	}
}

impl ToolCallState {
	/// The one place a change that adds fields older workers reject declares the
	/// capability those fields need.
	fn required_capabilities(&self) -> impl IntoIterator<Item = PersistedCapability> {
		// A persisted Tool Batch end (#119) is rejected by workers built before it.
		self.batch_end
			.map(|_| PersistedCapability::ToolBatchState)
			.into_iter()
			.chain(
				self.connection
					.as_ref()
					.map(|_| PersistedCapability::ConnectionWait),
			)
			.chain(self.mcp.as_ref().map(|_| PersistedCapability::McpCallState))
			.chain(
				self.mcp
					.as_ref()
					.and_then(|checkpoint| checkpoint.sent.as_ref())
					.map(|_| PersistedCapability::McpSentRound),
			)
	}
}
impl RecoveryState {
	fn required_capabilities(&self) -> impl IntoIterator<Item = PersistedCapability> {
		// Degraded context (#234) is rejected by workers built before it. It is
		// omitted while empty, so a Run that never degraded needs nothing.
		(!self.degraded.is_empty())
			.then_some(PersistedCapability::DegradedContext)
			.into_iter()
			.chain(
				(!self.connection_waited.is_empty()
					|| self.connection_wait.is_some()
					|| !self.connection_refusals.is_empty())
				.then_some(PersistedCapability::ConnectionWait),
			)
	}
}
impl RunState {
	/// Every `ToolCallState` a suspended or running state continues from.
	fn tool_states(&self) -> Option<&ToolCallState> {
		match self {
			Self::ToolCall(s) => Some(s),
			Self::Waiting(wait) => match wait.as_ref() {
				WaitingState::Approval { resume, .. }
				| WaitingState::Reconciliation { resume, .. } => Some(resume),
				WaitingState::Timer {
					resume: ResumeState::ToolCall(s),
					..
				}
				| WaitingState::Human {
					resume: ResumeState::ToolCall(s),
					..
				} => Some(s),
				_ => None,
			},
			_ => None,
		}
	}
}

/// The capability names a persisted state needs, kept as wire strings so a name
/// written by a newer build stays representable (and unmet) on an older one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct StateRequirements(Vec<String>);
impl StateRequirements {
	/// Derived from the state itself, so it cannot drift from what is saved.
	pub fn of(state: &RunState, recovery: &RecoveryState) -> Self {
		let needed: BTreeSet<PersistedCapability> = state
			.tool_states()
			.into_iter()
			.flat_map(ToolCallState::required_capabilities)
			.chain(recovery.required_capabilities())
			.collect();
		Self(needed.into_iter().map(|c| c.name().to_owned()).collect())
	}
	/// Reads only the stored requirement of a `pending` envelope; the state is not decoded.
	pub fn from_pending(pending: &Value) -> crate::Result<Self> {
		match pending.get("requires") {
			None => Ok(Self::default()),
			Some(value) => {
				let mut names: Vec<String> = serde_json::from_value(value.clone())
					.map_err(|_| crate::Error::Invalid("invalid capability requirement".into()))?;
				names.sort();
				names.dedup();
				Ok(Self(names))
			}
		}
	}
	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}
	pub fn names(&self) -> &[String] {
		&self.0
	}
	/// Every name here is also in `stored`.
	pub fn covered_by(&self, stored: &Self) -> bool {
		self.0.iter().all(|name| stored.0.contains(name))
	}
	/// Requirements the given worker capabilities do not cover.
	pub fn unmet(&self, supported: &[PersistedCapability]) -> Vec<String> {
		self.0
			.iter()
			.filter(|name| !supported.iter().any(|c| c.name() == name.as_str()))
			.cloned()
			.collect()
	}
}

/// Why a Run that no worker currently holds is not being claimed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LeaseWait {
	/// Only a worker that understands these capabilities may claim the Run.
	CapableWorker { capabilities: Vec<String> },
}

/// A serde failure that means the writer knew a field or variant this build does not.
pub(super) fn is_unknown_shape(error: &serde_json::Error) -> bool {
	let message = error.to_string();
	error.is_data()
		&& (message.starts_with("unknown field") || message.starts_with("unknown variant"))
}
