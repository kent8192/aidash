// Serializable contracts for registry.

pub use aidash_domain::model::{MediaRouteEvidence, ModelConfig, ReasoningEffort};

pub use aidash_domain::registry::{EntityRef, Entry};

pub use aidash_domain::registry::{Search, SkillFile};

pub use aidash_domain::registry::{AgentConfig, AgentPage};

pub use aidash_domain::registry::{ClusterConfig, CompactorConfig, Package};

/// One version in the Tenant's Registry with its approval state and history.
/// Discovery and execution see only approved versions; this view sees all.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct RegistryVersion {
	pub entry: Entry,
	/// Whether the Tenant must approve it before use: every Tenant definition,
	/// and the Sandboxed, research and Decider built-ins.
	pub needs_approval: bool,
	pub approval: Option<VersionApproval>,
	pub history: Vec<ApprovalRecord>,
}
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct VersionApproval {
	pub enabled: bool,
	pub revision: i64,
}
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct ApprovalRecord {
	pub revision: i64,
	pub enabled: bool,
	pub actor: String,
	pub created_at: chrono::DateTime<chrono::Utc>,
}
/// One page of the Tenant's Registry versions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct VersionPage {
	#[serde(default)]
	pub offset: u64,
}
