use serde::Deserialize;
// Serializable contracts for authorization.

use crate::registry::{EntityRef, Search};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InspectInput {
	#[serde(default)]
	pub task_id: Option<uuid::Uuid>,
	pub tenant: String,
	pub subject: String,
	pub agent: EntityRef,
	pub requirements: Search,
	#[serde(default)]
	pub compactor: Option<EntityRef>,
}

pub(crate) use aidash_domain::federation::execution::{Definition, Inspection};
