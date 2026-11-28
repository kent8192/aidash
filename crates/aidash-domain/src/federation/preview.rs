//! Scoped Preview requests between a viewer's Node and the Run's hosting Node.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
	Live { live_preview_id: Uuid },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketRequest {
	pub run_id: Uuid,
	pub target: Target,
	/// V's dashboard origin; H compares it with V's pinned Node Registration.
	pub frame_ancestor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Ticket {
	pub url: String,
	pub expires_at: DateTime<Utc>,
	/// V can display this domain; the desktop validates it from V's API itself.
	pub preview_domain: String,
}

/// Read-only scope selection; it never contains a viewing address or ticket.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
	pub run_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Live {
	pub id: Uuid,
	pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Inspection {
	pub live_preview: Option<Live>,
	pub preview_available: bool,
}
