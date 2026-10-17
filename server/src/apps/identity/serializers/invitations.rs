//! Contracts of Invitations: issuing them, and opening and redeeming a link.
use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Deserialize, JsonSchema, Validate, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvitationRequest {
	/// The Assignable Groups to propose; `members` when omitted.
	#[serde(default)]
	pub(crate) groups: Option<BTreeSet<String>>,
	/// How long the link lasts, at most 7 days (the default).
	#[serde(default)]
	pub(crate) expires_in_seconds: Option<i64>,
}

/// An issued Invitation. `link` and `secret` are shown only in this response.
#[derive(Serialize, JsonSchema)]
pub(crate) struct IssuedInvitation {
	pub(crate) id: Uuid,
	pub(crate) tenant: String,
	pub(crate) link: String,
	pub(crate) secret: String,
	pub(crate) groups: BTreeSet<String>,
	pub(crate) expires_at: DateTime<Utc>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct InvitationView {
	pub(crate) id: Uuid,
	pub(crate) groups: BTreeSet<String>,
	pub(crate) created_at: DateTime<Utc>,
	pub(crate) expires_at: DateTime<Utc>,
	/// `active`, `redeemed`, `revoked` or `expired`.
	pub(crate) state: String,
	pub(crate) registration_id: Option<Uuid>,
}

/// A link as its holder presents it.
#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvitationLink {
	#[validate(length(min = 1, max = 256))]
	pub(crate) tenant: String,
	#[validate(length(min = 1, max = 256))]
	pub(crate) secret: String,
}

/// What a signed-in person sees before requesting to join.
#[derive(Serialize, JsonSchema)]
pub(crate) struct InvitationPreview {
	pub(crate) tenant: String,
	pub(crate) label: Option<String>,
}
