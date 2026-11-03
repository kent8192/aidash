//! Serializable publication and private knowledge values.
use super::{EntityRef, Entry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClusterConfig {
	pub coordinator: EntityRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Package {
	pub entity: Entry,
	pub author: String,
	pub permissions: Vec<String>,
	#[serde(default)]
	pub dependencies: Vec<EntityRef>,
	/// The Tenant Secrets the package's tool names, which whoever installs it
	/// fills from their own Tenant (ADR 0051). Empty for most packages.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDocument {
	pub name: String,
	pub media_type: String,
	/// Text extracted locally for text-based documents; images keep no extracted text.
	#[serde(default)]
	pub text: String,
	/// Original image bytes, base64-encoded for the private registration API.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub data_base64: Option<String>,
	/// Immutable metadata used to verify an image without reading its bytes.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	#[schemars(skip)]
	pub image_digest: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	#[schemars(skip)]
	pub image_size: Option<usize>,
}
