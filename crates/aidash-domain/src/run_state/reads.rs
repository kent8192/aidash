//! What keeps a Run from a reader, without disclosing what it read.
use crate::registry::EntityRef;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The first of a Run's recorded reads that stops a reader whose own policy
/// allows the Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunReadsBlock {
	/// A definition the Run read has no approval on this Node. It is named only
	/// to a holder of `registry.read` on it.
	DefinitionNotApproved {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		definition: Option<EntityRef>,
	},
	/// A definition the Run read is approved, but this reader may not read it.
	DefinitionNotReadable,
	/// Something the Run read from another Node could not be confirmed.
	RemoteRead,
	/// A semantic index the Run read is no longer available to this reader.
	SemanticRead,
	/// A record of this kind that the Run read is no longer visible to this reader.
	Source { source: String },
}

impl fmt::Display for RunReadsBlock {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::DefinitionNotApproved {
				definition: Some(definition),
			} => write!(
				f,
				"{}@{}, which this Run read, is no longer approved on this Node",
				definition.id, definition.version
			),
			Self::DefinitionNotApproved { definition: None } => {
				f.write_str("a definition this Run read is no longer approved on this Node")
			}
			Self::DefinitionNotReadable => {
				f.write_str("your permissions do not allow a definition this Run read")
			}
			Self::RemoteRead => {
				f.write_str("something this Run read from another Node could not be confirmed")
			}
			Self::SemanticRead => {
				f.write_str("a semantic index this Run read is no longer available to you")
			}
			Self::Source { source } => {
				write!(f, "a {source} this Run read is no longer visible to you")
			}
		}
	}
}
