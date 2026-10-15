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
}

/// One research provider this Node knows and whether it can dispatch it.
#[derive(Serialize, JsonSchema)]
pub struct ResearchProvider {
	pub provider: String,
	pub operations: Vec<String>,
	/// `false` until an Operator sets the provider up on this Node.
	pub available: bool,
}

/// The research providers of this Node and the per-Run limits every Run here
/// is held to.
#[derive(Serialize, JsonSchema)]
pub struct ResearchProviders {
	pub providers: Vec<ResearchProvider>,
	pub limits: ResearchLimits,
}
