//! Finite domain states shared by business rules and persistence.

use reinhardt::ModelEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum DefinitionKind {
	#[default]
	#[model_enum(value = "agent")]
	#[serde(rename = "agent")]
	Agent,
	#[model_enum(value = "model")]
	#[serde(rename = "model")]
	Model,
	#[model_enum(value = "tool")]
	#[serde(rename = "tool")]
	Tool,
	#[model_enum(value = "skill")]
	#[serde(rename = "skill")]
	Skill,
	#[model_enum(value = "cluster")]
	#[serde(rename = "cluster")]
	Cluster,
	#[model_enum(value = "node")]
	#[serde(rename = "node")]
	Node,
	#[model_enum(value = "compactor")]
	#[serde(rename = "compactor")]
	Compactor,
	#[model_enum(value = "embedding")]
	#[serde(rename = "embedding")]
	Embedding,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum RegistryAgentResourceRefRequiredKind {
	#[default]
	#[model_enum(value = "tool")]
	#[serde(rename = "tool")]
	Tool,
	#[model_enum(value = "skill")]
	#[serde(rename = "skill")]
	Skill,
	#[model_enum(value = "cluster")]
	#[serde(rename = "cluster")]
	Cluster,
}

// Reinhardt #6456: use the declared storage codec for composite identity metadata.
// Remove these implementations when ModelEnum composite identities are generated
// by the framework from DatabaseField without separate PkValue/Display bounds.
impl From<&RegistryAgentResourceRefRequiredKind> for reinhardt::db::orm::composite_pk::PkValue {
	fn from(value: &RegistryAgentResourceRefRequiredKind) -> Self {
		Self::String(
			reinhardt::db::orm::DatabaseField::encode_database(value)
				.expect("ModelEnum encodes declared constant storage values"),
		)
	}
}
impl std::fmt::Display for RegistryAgentResourceRefRequiredKind {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let value = reinhardt::db::orm::DatabaseField::encode_database(self)
			.map_err(|_| std::fmt::Error)?;
		f.write_str(&value)
	}
}
