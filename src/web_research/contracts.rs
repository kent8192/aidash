use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub const MAX_ENVELOPE: usize = 32 * 1024;
pub const MAX_DOWNLOAD: usize = 10 * 1024 * 1024;
pub const MAX_TEXT: usize = 1024 * 1024;
pub const MAX_CACHE: u64 = 20 * 1024 * 1024;
pub const MAX_OBSERVATIONS: u64 = 200;
pub const MAX_OBSERVATION_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Open {
	pub url: Option<String>,
	pub source_id: Option<Uuid>,
	pub document_id: Option<Uuid>,
	pub cursor: Option<Uuid>,
	pub max_bytes: Option<u32>,
}
impl Open {
	pub(crate) fn validate(&self) -> Result<usize> {
		if usize::from(self.url.is_some())
			+ usize::from(self.source_id.is_some())
			+ usize::from(self.document_id.is_some())
			!= 1 || (self.cursor.is_some() && self.document_id.is_none())
		{
			return Err(Error::Invalid(
				"web_open requires exactly one selector; cursors require a document".into(),
			));
		}
		let limit = self.max_bytes.unwrap_or(16384) as usize;
		if !(1..=24576).contains(&limit) {
			return Err(Error::Invalid("invalid web_open byte limit".into()));
		}
		Ok(limit)
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Find {
	pub document_id: Uuid,
	pub query: String,
	#[serde(default)]
	pub case_sensitive: bool,
	pub max_matches: Option<u8>,
	pub cursor: Option<Uuid>,
}
impl Find {
	pub(crate) fn validate(&self) -> Result<usize> {
		let maximum = self.max_matches.unwrap_or(10) as usize;
		if self.query.trim().is_empty()
			|| self.query.len() > 1024
			|| self.query.chars().count() > 256
			|| self.query.chars().any(char::is_control)
			|| !(1..=20).contains(&maximum)
		{
			return Err(Error::Invalid(
				"invalid web_find literal or match limit".into(),
			));
		}
		Ok(maximum)
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DisclosureDecision {
	pub expected_revision: i64,
	pub request_digest: String,
	pub allow_once: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextLabel {
	pub expected_digest: String,
	/// Classifies all currently observed inputs, including instructions and memory.
	pub public: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Revision {
	pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Start {
	pub idempotency_key: Uuid,
	pub agent: crate::registry::EntityRef,
	pub description: String,
	/// An authenticated user's exact URL action, never an Agent-provided grant.
	pub open_url: Option<String>,
}

pub(crate) fn envelope(operation: &str, status: &str, data: Value) -> Value {
	json!({"version":1,"operation":operation,"status":status,"data":data,
		"limits":{"max_bytes":MAX_ENVELOPE,"truncated":false,"continuation":null}})
}
pub(crate) fn failure(operation: &str, status: &str, code: &str) -> Value {
	json!({"version":1,"operation":operation,"status":status,
		"error":{"code":code,"explanation":"The research operation could not complete under its current authority and limits.","retryable":false},
		"limits":{"max_bytes":MAX_ENVELOPE,"truncated":false,"continuation":null}})
}
pub(crate) fn is_web(name: &str) -> bool {
	matches!(name, "web_search" | "web_open" | "web_find")
}
