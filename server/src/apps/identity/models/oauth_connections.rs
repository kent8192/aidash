//! OAuth Connection metadata and browser-bound pending authorizations (ADR 0100).
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[model(app_label = "identity", table_name = "oauth_connections")]
#[derive(Serialize, Deserialize)]
pub struct OAuthConnection {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text")]
	pub resource: String,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field]
	pub client: Json<aidash_domain::oauth_connections::ClientRef>,
	#[field]
	pub scopes: Vec<String>,
	#[field(field_type = "text")]
	pub secret_resource: String,
	#[field(field_type = "text")]
	pub pinned_version: String,
	#[field(field_type = "text")]
	pub state: String,
	#[field(null = true)]
	pub access_expires_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub refresh_expires_at: Option<DateTime<Utc>>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub revision: i64,
}
#[model(app_label = "identity", table_name = "oauth_authorizations")]
#[derive(Serialize, Deserialize)]
pub struct OAuthAuthorization {
	#[field(primary_key = true, field_type = "text")]
	pub state_hash: String,
	#[field(field_type = "text")]
	pub browser_hash: String,
	#[field]
	pub session_id: Uuid,
	#[field(field_type = "text")]
	pub context: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text")]
	pub resource: String,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field]
	pub client: Json<aidash_domain::oauth_connections::ClientRef>,
	#[field]
	pub scopes: Vec<String>,
	#[field(field_type = "text")]
	pub pkce_verifier: String,
	#[field(field_type = "text")]
	pub redirect_uri: String,
	#[field(field_type = "text")]
	pub return_to: String,
	#[field]
	pub connection: Uuid,
	#[field]
	pub revision: i64,
	#[field]
	pub expires_at: DateTime<Utc>,
}
#[model(app_label = "identity", table_name = "oauth_dynamic_clients")]
#[derive(Serialize, Deserialize)]
pub struct OAuthDynamicClient {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field(field_type = "text")]
	pub redirect_uri: String,
	#[field(field_type = "text")]
	pub client_id: String,
	#[field(field_type = "text")]
	pub secret_resource: String,
	#[field(field_type = "text")]
	pub pinned_version: String,
}
