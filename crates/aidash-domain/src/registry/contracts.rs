//! Serializable publication and private knowledge values.
use super::{EntityRef, Entry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClusterConfig {
	pub coordinator: EntityRef,
}

/// Explicit transport and bounds for System One probability classification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactorConfig {
	pub provider: String,
	pub endpoint: String,
	pub model: String,
	pub credential_env: String,
	pub max_request_bytes: usize,
	pub max_questions: usize,
	pub max_response_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Package {
	pub entity: Entry,
	pub author: String,
	pub permissions: Vec<String>,
	#[serde(default)]
	pub dependencies: Vec<EntityRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDocument {
	pub name: String,
	pub media_type: String,
	/// Text extracted locally in the browser; source binaries are not retained.
	pub text: String,
}
