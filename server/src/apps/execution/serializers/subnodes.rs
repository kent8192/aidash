//! Person-facing Subnode summaries and their member Runs.
use aidash_domain::{registry::EntityRef, subnodes::Allowance};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize, JsonSchema)]
pub struct SubnodeMember {
	pub agent_id: String,
	pub agent_version: String,
	pub added_by_run_id: Uuid,
	pub added_at: DateTime<Utc>,
}
#[derive(Serialize, JsonSchema)]
pub struct MemberRun {
	pub id: Uuid,
	pub agent: EntityRef,
	pub state: String,
	pub control: String,
	pub task_id: Uuid,
}
#[derive(Serialize, JsonSchema)]
pub struct SubnodeView {
	pub id: Uuid,
	pub title: Option<String>,
	pub workspace_id: Uuid,
	pub leader: EntityRef,
	pub members: Vec<SubnodeMember>,
	pub allowance: Allowance,
	pub state: String,
	pub created_at: DateTime<Utc>,
	pub ended_at: Option<DateTime<Utc>>,
	pub ended_reason: Option<String>,
	pub member_runs: Vec<MemberRun>,
}
#[derive(Serialize, JsonSchema)]
pub struct Dissolved {
	pub dissolved: bool,
}
