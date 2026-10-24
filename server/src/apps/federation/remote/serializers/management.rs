//! Management API contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct DelegateInput {
	pub(crate) node_id: String,
	pub(crate) agent: EntityRef,
}
