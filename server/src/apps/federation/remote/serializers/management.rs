//! Management API contracts.
use crate::apps::federation::peer::serializers::management::RemoteControl;
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct DelegateInput {
	pub(crate) node_id: String,
	pub(crate) agent: EntityRef,
}
#[derive(Deserialize, JsonSchema)]
pub(crate) struct RemoteActionInput {
	pub(crate) node_id: String,
	pub(crate) control: RemoteControl,
}
