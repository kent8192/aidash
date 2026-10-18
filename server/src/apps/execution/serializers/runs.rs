//! API response contracts.
use crate::{
	domain::{HumanRequest, RunInspection},
	store::Invocation,
};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, schemars::JsonSchema)]
pub struct RunDetails {
	pub run: crate::domain::RunInspection,
	pub invocations: Vec<Invocation>,
	pub memory: Option<aidash_domain::memory::Binding>,
	/// Each current route lists MIME types accepted together by the run model.
	pub media_input_routes: Vec<Vec<String>>,
}

#[derive(Serialize, JsonSchema)]
pub struct PeerObservation {
	pub node_id: String,
	pub runs: Vec<RunInspection>,
	pub human_requests: Vec<HumanRequest>,
	pub invocations: Vec<Invocation>,
}

/// Per-Run research limits; reaching one blocks dispatch and disclosure.
#[derive(Serialize, JsonSchema)]
pub struct ResearchLimits {
	pub search_attempts: u32,
	pub fetch_attempts: u32,
	pub sources: u32,
	pub snapshots: u32,
	pub snapshot_bytes: u64,
}

impl From<aidash_domain::research::evidence::Limits> for ResearchLimits {
	fn from(limits: aidash_domain::research::evidence::Limits) -> Self {
		Self {
			search_attempts: limits.search_attempts,
			fetch_attempts: limits.fetch_attempts,
			sources: limits.sources,
			snapshots: limits.snapshots,
			snapshot_bytes: limits.snapshot_bytes,
		}
	}
}

/// What one Run has used of its research limits.
#[derive(Serialize, JsonSchema)]
pub struct ResearchUsage {
	pub search_attempts: u32,
	pub fetch_attempts: u32,
	pub sources: u32,
	pub snapshots: u32,
	pub snapshot_bytes: u64,
}

impl From<aidash_domain::research::evidence::RunUsage> for ResearchUsage {
	fn from(usage: aidash_domain::research::evidence::RunUsage) -> Self {
		Self {
			search_attempts: usage.search_attempts,
			fetch_attempts: usage.fetch_attempts,
			sources: usage.sources,
			snapshots: usage.snapshots,
			snapshot_bytes: usage.snapshot_bytes,
		}
	}
}

/// A Run's research usage against its limits, for anyone who may read the Run.
#[derive(Serialize, JsonSchema)]
pub struct RunResearch {
	pub limits: ResearchLimits,
	pub usage: ResearchUsage,
	/// Recorded prices of the Run's charged attempts: an estimate, not an
	/// invoice.
	pub estimated_micro_usd: u64,
	/// Attempts the provider may have performed whose outcome is unknown.
	/// They are never sent again automatically.
	pub uncertain_attempts: u32,
	/// Attempts sent and not yet settled.
	pub in_flight_attempts: u32,
	/// The pages people asked the Run to read, oldest first. Each only asked
	/// for a Page reading; none approved a Web disclosure.
	pub url_intents: Vec<RunUrlIntent>,
	/// When the Run's research evidence was revoked (ADR 0046). Who revoked it
	/// and why is in its revocation record, readable only by its audience.
	pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
	/// Whether the reader may revoke the Run's research evidence.
	pub can_revoke: bool,
	/// Whether the reader may read the Run's revocation record.
	pub can_read_revocation: bool,
}

/// The pages one input asked a Run to read.
#[derive(Serialize, JsonSchema)]
pub struct RunUrlIntent {
	/// `task` for the input that started or queued the Run, `run_message`
	/// for a later message steering it.
	pub input: UrlIntentInput,
	pub pages: Vec<String>,
	pub requested_by: String,
	pub requested_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UrlIntentInput {
	Task,
	RunMessage,
}

/// A tool definition of the reader's Tenant, by identifier and version.
#[derive(Serialize, JsonSchema)]
pub struct DefinitionRef {
	pub id: String,
	pub version: String,
}

/// One research operation of a provider and the tool definitions through
/// which the reader's Tenant may bind it here.
#[derive(Serialize, JsonSchema)]
pub struct ResearchProviderOperation {
	pub operation: String,
	/// The Tenant's tool definitions of this operation that have a Catalog
	/// Approval. A Run can use the operation only through one of them.
	pub approved_definitions: Vec<DefinitionRef>,
}

/// One Research provider this Deployment knows.
#[derive(Serialize, JsonSchema)]
pub struct ResearchProvider {
	pub provider: String,
	/// Whether whoever runs the Deployment has set the provider up, with
	/// current account terms where it has any. It never says what is missing.
	pub set_up: bool,
	pub operations: Vec<ResearchProviderOperation>,
}

/// The Research providers of this Deployment and the per-Run limits every
/// Run is held to.
#[derive(Serialize, JsonSchema)]
pub struct ResearchProviders {
	pub providers: Vec<ResearchProvider>,
	pub limits: ResearchLimits,
}
