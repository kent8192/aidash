//! Finite domain states shared by business rules and persistence.

use reinhardt::ModelEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum HumanRequestKind {
	#[default]
	#[model_enum(value = "QUESTION")]
	#[serde(rename = "QUESTION")]
	Question,
	#[model_enum(value = "APPROVAL_REQUIRED")]
	#[serde(rename = "APPROVAL_REQUIRED")]
	ApprovalRequired,
	#[model_enum(value = "CONFIRMATION")]
	#[serde(rename = "CONFIRMATION")]
	Confirmation,
	#[model_enum(value = "INFORMATION_REQUEST")]
	#[serde(rename = "INFORMATION_REQUEST")]
	InformationRequest,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum InvocationStatus {
	#[default]
	#[model_enum(value = "STARTED")]
	#[serde(rename = "STARTED")]
	Started,
	#[model_enum(value = "COMPLETED")]
	#[serde(rename = "COMPLETED")]
	Completed,
	#[model_enum(value = "UNCERTAIN")]
	#[serde(rename = "UNCERTAIN")]
	Uncertain,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum RunControl {
	#[default]
	#[model_enum(value = "ACTIVE")]
	#[serde(rename = "ACTIVE")]
	Active,
	#[model_enum(value = "PAUSED")]
	#[serde(rename = "PAUSED")]
	Paused,
	#[model_enum(value = "CANCELLED")]
	#[serde(rename = "CANCELLED")]
	Cancelled,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum RunPhase {
	#[default]
	#[model_enum(value = "READY")]
	#[serde(rename = "READY")]
	Ready,
	#[model_enum(value = "THINKING")]
	#[serde(rename = "THINKING")]
	Thinking,
	#[model_enum(value = "TOOL_CALL")]
	#[serde(rename = "TOOL_CALL")]
	ToolCall,
	#[model_enum(value = "WAITING")]
	#[serde(rename = "WAITING")]
	Waiting,
	#[model_enum(value = "COMPLETED")]
	#[serde(rename = "COMPLETED")]
	Completed,
	#[model_enum(value = "FAILED")]
	#[serde(rename = "FAILED")]
	Failed,
	#[model_enum(value = "CANCELLED")]
	#[serde(rename = "CANCELLED")]
	Cancelled,
}
