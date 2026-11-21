//! Tenant Instructions and Node Instructions (#1010).
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One stored text; a Tenant or Node that never stored one reads as an empty
/// text at revision 0.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "StandingInstructionsText")]
pub struct Instructions {
	pub text: String,
	pub revision: i64,
	pub updated_at: Option<DateTime<Utc>>,
	pub updated_by: Option<String>,
}

/// Replaces the text when `expected_revision` is still current (409 otherwise).
/// At most 16,384 bytes and no NUL characters.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "StandingInstructionsTextUpdate")]
pub struct Update {
	pub text: String,
	pub expected_revision: i64,
}

impl From<crate::apps::identity::repositories::standing_instructions::Stored> for Instructions {
	fn from(stored: crate::apps::identity::repositories::standing_instructions::Stored) -> Self {
		Self {
			text: stored.text,
			revision: stored.revision,
			updated_at: stored.updated_at,
			updated_by: stored.updated_by,
		}
	}
}
