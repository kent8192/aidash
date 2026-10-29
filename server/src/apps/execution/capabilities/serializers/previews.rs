//! Typed contracts for Preview tickets and availability.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct PreviewTicketInput {
	pub area_id: Uuid,
	pub file_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PreviewTicket {
	pub url: String,
	pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PreviewAvailability {
	pub available: bool,
	pub preview_domain: Option<String>,
}

impl From<aidash_application::preview::Availability> for PreviewAvailability {
	fn from(value: aidash_application::preview::Availability) -> Self {
		Self {
			available: value.available,
			preview_domain: value.preview_domain,
		}
	}
}

impl From<aidash_application::preview::Issued> for PreviewTicket {
	fn from(value: aidash_application::preview::Issued) -> Self {
		Self {
			url: value.url,
			expires_at: value.expires_at,
		}
	}
}

/// One Run's Live preview as the dashboard reads it.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LivePreviewStatus {
	pub live_preview: Option<LivePreview>,
	/// Whether this viewer may stop it: the Run's requester or whoever may
	/// cancel the Run (ADR 0070). The dashboard shows Stop only then.
	pub may_stop: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LivePreview {
	pub id: Uuid,
	pub state: String,
	pub stop_reason: Option<String>,
	pub area_id: Uuid,
	pub revision: i64,
	pub port: u16,
	pub idle_seconds: i64,
	pub absolute_seconds: i64,
	pub started_at: DateTime<Utc>,
	pub last_request_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
}

impl From<aidash_application::preview::live::Status> for LivePreview {
	fn from(value: aidash_application::preview::live::Status) -> Self {
		Self {
			id: value.id,
			state: value.state.as_str().to_owned(),
			stop_reason: value.stop_reason.map(|reason| reason.as_str().to_owned()),
			area_id: value.area_id,
			revision: value.revision,
			port: value.port,
			idle_seconds: value.limits.idle_seconds,
			absolute_seconds: value.limits.absolute_seconds,
			started_at: value.started_at,
			last_request_at: value.last_request_at,
			expires_at: value.expires_at,
		}
	}
}

impl LivePreviewStatus {
	pub fn of(value: Option<aidash_application::preview::live::Status>, may_stop: bool) -> Self {
		Self {
			live_preview: value.map(Into::into),
			may_stop,
		}
	}
}

/// `POST /api/previews/live-tickets`: one Live preview's one-time ticket.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct LivePreviewTicketInput {
	pub live_preview_id: Uuid,
}

/// A Tenant's lowered Live preview limits and the Deployment's ceilings.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TenantPreviewLimits {
	pub tenant: String,
	pub idle_seconds: i64,
	pub absolute_seconds: i64,
	pub revision: i64,
	pub ceiling_idle_seconds: i64,
	pub ceiling_absolute_seconds: i64,
}

/// `PUT /api/preview-limits/{tenant}`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct PreviewLimitInput {
	pub tenant: String,
	pub idle_seconds: i64,
	pub absolute_seconds: i64,
	pub expected_revision: i64,
}
