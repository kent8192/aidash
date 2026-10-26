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
