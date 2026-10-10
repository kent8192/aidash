//! Provider Credential metadata only; Key Material is never an ORM field.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[model(app_label = "identity", table_name = "provider_credentials")]
#[derive(Serialize, Deserialize)]
pub struct ProviderCredential {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field(field_type = "text")]
	pub base_url: String,
	#[field(field_type = "text")]
	pub secret_resource: String,
	#[field(field_type = "text", null = true)]
	pub pinned_version: Option<String>,
	#[field(field_type = "text")]
	pub fingerprint: String,
	#[field(field_type = "text")]
	pub last4: String,
	#[field(field_type = "text")]
	pub state: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub rotated_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub revoked_at: Option<DateTime<Utc>>,
	#[field]
	pub revision: i64,
}
#[model(app_label = "identity", table_name = "provider_credential_bindings")]
#[derive(Serialize, Deserialize)]
pub struct ProviderCredentialBinding {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field(null = true)]
	pub provider_credential_id: Option<Uuid>,
	#[field]
	pub revision: i64,
}

/// Local-only admission evidence; never serialized into Run or peer messages.
#[model(app_label = "identity", table_name = "run_provider_credentials")]
#[derive(Serialize, Deserialize)]
pub struct RunProviderCredential {
	#[field(primary_key = true, field_type = "text")]
	pub id: String,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub provider: String,
	#[field]
	pub provider_credential_id: Uuid,
}
