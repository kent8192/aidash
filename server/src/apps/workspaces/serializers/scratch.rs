//! Scratch Space request payloads.
use aidash_domain::{
	nodes::NodeName,
	scratch::{HostAllowlist, ScratchCeilings, ScratchSettings},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchCreate {
	pub node: NodeName,
	pub name: String,
	#[serde(default)]
	pub settings: Option<ScratchSettings>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchMember {
	pub subject: String,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchSettingsUpdate {
	pub revision: i64,
	pub settings: ScratchSettings,
}
#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ScratchList {
	pub node: NodeName,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ScratchLimits {
	pub ceilings: ScratchCeilings,
	pub tenant_hosts: HostAllowlist,
}
