//! Finite domain states shared by business rules and persistence.

use reinhardt::ModelEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum AuthorizationRunReadResourceKind {
	#[default]
	#[model_enum(value = "task")]
	#[serde(rename = "task")]
	Task,
	#[model_enum(value = "artifact")]
	#[serde(rename = "artifact")]
	Artifact,
	#[model_enum(value = "message")]
	#[serde(rename = "message")]
	Message,
	#[model_enum(value = "run")]
	#[serde(rename = "run")]
	Run,
	#[model_enum(value = "conversation")]
	#[serde(rename = "conversation")]
	Conversation,
	#[model_enum(value = "generation")]
	#[serde(rename = "generation")]
	Generation,
	#[model_enum(value = "workspace_events")]
	#[serde(rename = "workspace_events")]
	WorkspaceEvents,
}
