//! Exposure of Journal tools after an Offloaded Result is persisted.
use super::Context;
use serde::{Deserialize, Serialize};

/// Set once this Run has material outside its projection.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Offloaded {}

pub fn offloaded_any(context: &Context) -> bool {
	context.offloaded.is_some()
}
