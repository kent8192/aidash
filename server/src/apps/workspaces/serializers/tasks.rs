//! API response contracts.
use schemars::JsonSchema;
use serde::Deserialize;

pub use aidash_domain::workspaces::{ConversationResponse, TaskPage};

#[derive(Default, Deserialize, JsonSchema)]
pub struct PageQuery {
	#[serde(default)]
	pub offset: u64,
}
