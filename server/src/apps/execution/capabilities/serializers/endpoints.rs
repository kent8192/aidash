use serde::Deserialize;

// Serializable endpoints contracts.
use crate::apps::execution::capabilities::services::core::{contracts::*, management::*};

#[derive(Default, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
	pub(crate) cursor: Option<Uuid>,
}

#[derive(Default, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct AreaQuery {
	pub(crate) cursor: Option<Uuid>,
	pub(crate) thread_id: Option<Uuid>,
	pub(crate) workspace_id: Option<Uuid>,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct OutboundPage {
	pub(crate) items: Vec<OutboundStatus>,
	pub(crate) next_cursor: Option<Uuid>,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedRevision {
	#[validate(range(min = 0))]
	pub(crate) expected_revision: i64,
}

#[derive(Default, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Offset {
	pub(crate) offset: Option<u64>,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct DownloadChunk {
	pub(crate) file: FileEntry,
	pub(crate) offset: u64,
	pub(crate) data: String,
	pub(crate) next_offset: Option<u64>,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipientQuery {
	pub(crate) node_id: String,
	pub(crate) cursor: Option<Uuid>,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
pub(crate) struct ReferencePage {
	pub(crate) items: Vec<super::references::Reference>,
	pub(crate) next_cursor: Option<Uuid>,
}

use uuid::Uuid;
