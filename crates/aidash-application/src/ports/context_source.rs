//! Read adapters retain all authority checks; declarations only select behavior.
use crate::Result;
use aidash_domain::context::source::{DetailRequest, SourceBatch, SourceRequest};
use async_trait::async_trait;

/// Lifecycle positions at which the operator has enough information to collect
/// a source. Native adapter names and Registry kinds belong to the resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSourcePhase {
	Instructions,
	FixedData,
	SnapshotData,
	BudgetedData,
}

/// A provider-owned reference to one admitted source group. It is meaningful
/// only to the environment that compiled the plan for this Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSourceHandle(String);
impl ContextSourceHandle {
	pub fn new(value: String) -> Self {
		Self(value)
	}
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextSourceTarget {
	/// Only the host's trusted implicit Skill adapters use this target. Explicit
	/// Source declarations contribute attributed data instead.
	Instructions,
	Pinned(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSourceCache {
	None,
	Boundary,
	Revision,
	Retrieval,
}

/// A resolved plan retains the recorded source identities, but grants no
/// authority to read them. Providers repeat their normal checks when collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedContextSource {
	pub handle: ContextSourceHandle,
	pub phase: ContextSourcePhase,
	pub target: ContextSourceTarget,
	pub cache: ContextSourceCache,
	pub contribution_slot: String,
	pub sources: Vec<aidash_domain::registry::bindings::QualifiedRef>,
	pub refresh_each_step: bool,
}

/// Private data of the current execution boundary, never Registry configuration.
/// All providers receive the same frame; the operator knows no native adapter
/// discriminator and supplies bounded retrieval information only when available.
#[derive(Clone, Copy)]
pub struct ContextSourceFrame<'a> {
	pub task: &'a aidash_domain::Task,
	pub entry: &'a aidash_domain::registry::Entry,
	pub input_cutoff: i64,
	pub inputs: &'a [(aidash_domain::semantic::InputRead, String)],
	pub budget: usize,
	pub key: Option<&'a aidash_domain::context::sources::RetrievalKey>,
	pub degrade_on_outage: bool,
	pub prepared: &'a [super::execution::PreparedSkillSource],
}

#[async_trait]
pub trait ContextSourceProvider: Send + Sync {
	async fn fetch(&self, request: &SourceRequest) -> Result<SourceBatch>;
	async fn detail(&self, request: &DetailRequest) -> Result<SourceBatch>;
}
