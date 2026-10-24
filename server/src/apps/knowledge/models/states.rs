//! Finite domain states shared by business rules and persistence.

use reinhardt::ModelEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum SemanticEntryState {
	#[default]
	#[model_enum(value = "PENDING")]
	#[serde(rename = "PENDING")]
	Pending,
	#[model_enum(value = "READY")]
	#[serde(rename = "READY")]
	Ready,
	#[model_enum(value = "ERROR")]
	#[serde(rename = "ERROR")]
	Error,
	#[model_enum(value = "REVOKED")]
	#[serde(rename = "REVOKED")]
	Revoked,
	#[model_enum(value = "DELETED")]
	#[serde(rename = "DELETED")]
	Deleted,
}
