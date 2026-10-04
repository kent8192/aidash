//! Node-qualified dependency proofs use the existing Federation JSON contract.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const LIMIT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reference {
	Grant {
		node_id: String,
		execution_node: String,
		grant_id: Uuid,
		admission_id: Uuid,
	},
	Admission {
		node_id: String,
		home_node: String,
		grant_id: Uuid,
		admission_id: Uuid,
	},
	Registry {
		node_id: String,
		id: String,
		version: String,
		digest: String,
	},
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
	pub tenant: String,
	pub subject: String,
	pub reference: Reference,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Checked {
	pub visible: bool,
	pub pending: Vec<Reference>,
}

impl Reference {
	pub fn node(&self) -> &str {
		match self {
			Self::Grant { node_id, .. }
			| Self::Admission { node_id, .. }
			| Self::Registry { node_id, .. } => node_id,
		}
	}
}

impl Checked {
	/// A denial or oversized frontier never discloses dependency identities.
	pub fn disclosed(self) -> Self {
		if self.pending.len() > LIMIT {
			return Self {
				visible: false,
				pending: vec![],
			};
		}
		Self {
			visible: self.visible,
			pending: if self.visible { self.pending } else { vec![] },
		}
	}
}

#[cfg(test)]
mod tests;
