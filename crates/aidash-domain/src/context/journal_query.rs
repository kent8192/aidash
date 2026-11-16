//! Exposure of Journal tools after an Offloaded Result is persisted.
use super::Context;
use serde::{Deserialize, Serialize};

/// Set once this Run has material outside its projection.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Offloaded {}

/// Journal retrieval tools a Run receives as intrinsics once it has
/// material outside its projection, or a handed Journal range.
pub const TOOLS: &[&str] = &["journal_search", "journal_read", "journal_query"];

pub fn offloaded_any(context: &Context) -> bool {
	context.offloaded.is_some()
}
