// Serializable contracts for registry.

pub use aidash_domain::model::{MediaRouteEvidence, ModelConfig, ReasoningEffort};

pub use aidash_domain::registry::{EntityRef, Entry};

pub use aidash_domain::registry::{Search, SkillFile};

pub use aidash_domain::registry::{AgentConfig, AgentPage};

pub use aidash_domain::registry::{ClusterConfig, Package};

/// One version in the Tenant's Registry with its approval state and history.
/// Discovery and execution see only approved versions; this view sees every
/// version the caller may read.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct RegistryVersion {
	pub entry: Entry,
	/// Whether the Tenant must approve it before use: every Tenant definition,
	/// and the Sandboxed, research and Decider built-ins.
	pub needs_approval: bool,
	pub approval: Option<VersionApproval>,
	pub history: Vec<ApprovalRecord>,
	/// The agent subject an approval created, for an agent version (ADR 0062),
	/// shown to subjects who may approve the version.
	pub subject: Option<AgentSubject>,
}
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct AgentSubject {
	pub id: String,
	pub groups: std::collections::BTreeSet<String>,
	pub enabled: bool,
}
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct VersionApproval {
	pub enabled: bool,
	pub revision: i64,
	/// Who may use the version on this Node; absent means everyone the
	/// Tenant's policy allows (ADR 0095).
	pub restriction: Option<aidash_domain::identity::catalog::CatalogRestriction>,
}
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct ApprovalRecord {
	pub revision: i64,
	pub enabled: bool,
	pub actor: String,
	pub created_at: chrono::DateTime<chrono::Utc>,
	/// The restriction this revision recorded.
	pub restriction: Option<aidash_domain::identity::catalog::CatalogRestriction>,
}
/// One page of the versions the caller may read, in id and version order.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct RegistryVersions {
	pub versions: Vec<RegistryVersion>,
	/// The offset of the next page, absent after the last one.
	pub next_offset: Option<u64>,
}
/// One page of the Tenant's Registry versions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct VersionPage {
	/// The Node whose Catalog Approvals and Agent subjects the page reports.
	pub node: aidash_domain::nodes::NodeName,
	#[serde(default)]
	pub offset: u64,
}

/// Which MCP tool the registration form is about to register.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpToolHintRequest {
	pub endpoint: String,
	pub tool_name: String,
}
/// What an MCP server says about one of its own tools. It only prefills the
/// registration form; a definition version declares a Read-only operation
/// itself (ADR 0078).
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct McpToolHint {
	/// The tool's `annotations.readOnlyHint`, or null when the server gives none.
	pub read_only_hint: Option<bool>,
}
