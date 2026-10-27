//! Registered client metadata and Tenant scope ceilings; no secret payloads.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[model(app_label = "identity", table_name = "oauth_clients")]
#[derive(Serialize, Deserialize)]
pub struct OAuthClient {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text", null = true)]
	pub owner: Option<String>,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field(field_type = "text")]
	pub client_id: String,
	#[field(field_type = "text", null = true)]
	pub secret_resource: Option<String>,
	#[field(field_type = "text", null = true)]
	pub pinned_version: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub revision: i64,
}
#[model(app_label = "identity", table_name = "connection_scope_ceilings")]
#[derive(Serialize, Deserialize)]
pub struct ScopeCeiling {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub resource: String,
	#[field]
	pub scopes: Vec<String>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub revision: i64,
}
