//! Stopping a flow (#1012, ADR 0143).
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What happened to one thing a flow made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
	/// Cancelled, ended or paused now.
	Stopped,
	/// It had already ended, or was already paused.
	AlreadyStopped,
	/// Stopping it failed; `error` says why. The rest were still stopped.
	Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoppedItem {
	pub id: Uuid,
	pub outcome: StopOutcome,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
}

/// A Node the flow created. Stopping a flow leaves Nodes in place: they belong
/// to the Tenant, and a Tenant Administrator deletes them if they are unwanted.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FlowNode {
	pub name: String,
	pub label: String,
}

/// What `POST /api/tenants/{tenant}/flows/{root_task}/stop` stopped: every open
/// Run of the flow is cancelled, so is the work it handed to other
/// Deployments, its Subnodes end and its Duties pause.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FlowStopped {
	pub root_task: Uuid,
	/// How many Tasks the flow holds.
	pub tasks: usize,
	pub runs: Vec<StoppedItem>,
	/// Remote Grants that handed the flow's Tasks to another Deployment, by
	/// grant: each is revoked, its receiver cancels the Run, and the Task here
	/// is cancelled. A grant whose Task had ended is `already_stopped`.
	pub remote: Vec<StoppedItem>,
	pub subnodes: Vec<StoppedItem>,
	pub duties: Vec<StoppedItem>,
	/// Nodes the flow created, left in place.
	pub nodes: Vec<FlowNode>,
}
