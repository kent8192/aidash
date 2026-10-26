// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Authentication-owned records. No foreign keys point at product-owned tables.
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "authentication", table_name = "auth_accounts")]
#[derive(Serialize, Deserialize)]
pub struct AccountRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub document: Json<Value>,
}
#[model(app_label = "authentication", table_name = "auth_identities", constraints = [unique(fields = ["issuer", "subject"], name = "auth_identity_source")])]
#[derive(Serialize, Deserialize)]
pub struct IdentityRecord {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field]
	pub document: Json<Value>,
}
#[model(app_label = "authentication", table_name = "auth_sessions")]
#[derive(Serialize, Deserialize)]
pub struct SessionRecord {
	#[field(field_type = "text")]
	pub session_kind: String,
	#[field]
	pub expires_at: i64,
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub account_id: Uuid,
	#[field]
	pub identity_id: Uuid,
	#[field]
	pub document: Json<Value>,
}
#[model(app_label = "authentication", table_name = "auth_proofs")]
#[derive(Serialize, Deserialize)]
pub struct ProofRecord {
	#[field]
	pub expires_at: i64,
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub document: Json<Value>,
}
#[model(app_label = "authentication", table_name = "auth_flows")]
#[derive(Serialize, Deserialize)]
pub struct FlowRecord {
	#[field(null = true)]
	pub expires_at: Option<i64>,
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub browser_digest: String,
	#[field]
	pub document: Json<Value>,
}
#[model(app_label = "authentication", table_name = "auth_token_bindings")]
#[derive(Serialize, Deserialize)]
pub struct TokenBinding {
	#[field]
	pub expires_at: i64,
	#[field(primary_key = true, field_type = "text")]
	pub digest: String,
	#[field]
	pub session_id: Uuid,
	#[field(field_type = "text")]
	pub kind: String,
}
#[model(app_label = "authentication", table_name = "auth_rate_limits")]
#[derive(Serialize, Deserialize)]
pub struct RateLimit {
	#[field(primary_key = true, field_type = "text")]
	pub bucket: String,
	#[field]
	pub minute: i64,
	#[field]
	pub attempts: i64,
}
#[model(app_label = "authentication", table_name = "auth_refresh_responses")]
#[derive(Serialize, Deserialize)]
pub struct RefreshResponse {
	#[field]
	pub expires_at: i64,
	#[field(primary_key = true, field_type = "text")]
	pub digest: String,
	#[field]
	pub document: Json<Value>,
}
#[model(app_label = "authentication", table_name = "auth_source_observations")]
#[derive(Serialize, Deserialize)]
pub struct SourceObservation {
	#[field(primary_key = true)]
	pub identity_id: Uuid,
	#[field(null = true)]
	pub valid_since: Option<chrono::DateTime<chrono::Utc>>,
}
/// A verified upstream snapshot owned by one short-lived proof or session.
/// There is deliberately no Account key or Account-level profile fallback.
#[model(app_label = "authentication", table_name = "auth_ephemeral_profiles")]
#[derive(Serialize, Deserialize)]
pub struct EphemeralProfile {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub identity_id: Uuid,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub auth_time: i64,
	#[field]
	pub expires_at: i64,
	#[field(null = true, field_type = "text")]
	pub verified_email: Option<String>,
	#[field(null = true, field_type = "text")]
	pub display_name: Option<String>,
}
