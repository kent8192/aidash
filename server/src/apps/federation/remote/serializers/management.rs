//! Management API contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct DelegateInput {
	#[serde(default)]
	pub(crate) task_model: Option<aidash_domain::TaskModel>,
	pub(crate) node_id: String,
	pub(crate) agent: EntityRef,
}
