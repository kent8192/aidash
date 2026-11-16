//! Paging of an audit history.
use schemars::JsonSchema;

/// One page of audit history, newest first.
#[derive(Debug, serde::Deserialize, JsonSchema)]
pub struct HistoryPage {
	#[serde(default)]
	pub offset: u64,
	/// At most 200; 50 when absent.
	#[serde(default = "history_limit")]
	pub limit: u32,
}

fn history_limit() -> u32 {
	50
}
