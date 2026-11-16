//! Single-use, expiring Invitations. Only a digest of the secret is stored.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "tenant_invitations")]
#[derive(Serialize, Deserialize)]
pub struct TenantInvitation {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub token_hash: Vec<u8>,
	/// The Assignable Groups the issuer proposes; approval starts from them.
	#[field]
	pub groups: Json<Value>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub expires_at: DateTime<Utc>,
	/// `deployment_administrator` or `tenant_administrator`.
	#[field(field_type = "text")]
	pub issued_by_kind: String,
	#[field(null = true)]
	pub issued_by_identity: Option<uuid::Uuid>,
	/// The Mapping whose authority must still allow approval at redemption.
	#[field(null = true)]
	pub issued_by_mapping: Option<uuid::Uuid>,
	/// Domain-separated fingerprint of a Deployment Administrator bearer.
	#[field(null = true)]
	pub issued_by_token_hash: Option<Vec<u8>>,
	#[field(default = false)]
	pub join_directly: bool,
	#[field(null = true)]
	pub revoked_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub redeemed_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub redeemed_by: Option<uuid::Uuid>,
	#[field(null = true)]
	pub registration_id: Option<uuid::Uuid>,
}
