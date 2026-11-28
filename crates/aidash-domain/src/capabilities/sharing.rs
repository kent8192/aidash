//! Identifies the destination of a remote Thread Attachment delivery.
use serde::{Deserialize, Serialize};

pub use crate::media::Selection;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipient {
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
	pub thread_id: Uuid,
}
