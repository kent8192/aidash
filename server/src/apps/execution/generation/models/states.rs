//! Finite domain states shared by business rules and persistence.

use reinhardt::ModelEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum GenerationEmbeddingUsagePurpose {
	#[default]
	#[model_enum(value = "query")]
	#[serde(rename = "query")]
	Query,
	#[model_enum(value = "index")]
	#[serde(rename = "index")]
	Index,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum GenerationRequestStatus {
	#[default]
	#[model_enum(value = "PENDING_APPROVAL")]
	#[serde(rename = "PENDING_APPROVAL")]
	PendingApproval,
	#[model_enum(value = "QUEUED")]
	#[serde(rename = "QUEUED")]
	Queued,
	#[model_enum(value = "ACTIVE")]
	#[serde(rename = "ACTIVE")]
	Active,
	#[model_enum(value = "COMPLETED")]
	#[serde(rename = "COMPLETED")]
	Completed,
	#[model_enum(value = "DENIED")]
	#[serde(rename = "DENIED")]
	Denied,
	#[model_enum(value = "STOPPED")]
	#[serde(rename = "STOPPED")]
	Stopped,
	#[model_enum(value = "EXPIRED")]
	#[serde(rename = "EXPIRED")]
	Expired,
	#[model_enum(value = "FAILED")]
	#[serde(rename = "FAILED")]
	Failed,
	#[model_enum(value = "DELETED")]
	#[serde(rename = "DELETED")]
	Deleted,
}
