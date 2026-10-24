//! Finite domain states shared by business rules and persistence.

use reinhardt::ModelEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum ArtifactKind {
	#[default]
	#[model_enum(value = "text")]
	#[serde(rename = "text")]
	Text,
	#[model_enum(value = "json")]
	#[serde(rename = "json")]
	Json,
	#[model_enum(value = "file_reference")]
	#[serde(rename = "file_reference")]
	FileReference,
	#[model_enum(value = "code")]
	#[serde(rename = "code")]
	Code,
	#[model_enum(value = "structured_result")]
	#[serde(rename = "structured_result")]
	StructuredResult,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum ConversationTargetKind {
	#[default]
	#[model_enum(value = "agent")]
	#[serde(rename = "agent")]
	Agent,
	#[model_enum(value = "cluster")]
	#[serde(rename = "cluster")]
	Cluster,
}

#[derive(
	Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum, JsonSchema,
)]
#[model_enum(repr = "string")]
pub enum TaskStatus {
	#[default]
	#[model_enum(value = "OPEN")]
	#[serde(rename = "OPEN")]
	Open,
	#[model_enum(value = "CLAIMED")]
	#[serde(rename = "CLAIMED")]
	Claimed,
	#[model_enum(value = "RUNNING")]
	#[serde(rename = "RUNNING")]
	Running,
	#[model_enum(value = "COMPLETED")]
	#[serde(rename = "COMPLETED")]
	Completed,
	#[model_enum(value = "FAILED")]
	#[serde(rename = "FAILED")]
	Failed,
	#[model_enum(value = "BLOCKED")]
	#[serde(rename = "BLOCKED")]
	Blocked,
	#[model_enum(value = "CANCELLED")]
	#[serde(rename = "CANCELLED")]
	Cancelled,
	#[model_enum(value = "ABANDONED")]
	#[serde(rename = "ABANDONED")]
	Abandoned,
}
