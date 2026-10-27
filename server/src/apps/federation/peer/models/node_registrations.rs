//! A Node Registration (#241): one Node of a Tenant accepts one named Node of
//! another Tenant, and addresses outbound exchange to it. Its revisions are
//! kept in `node_registration_history`.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "node_registrations")]
#[derive(Serialize, Deserialize)]
pub struct NodeRegistration {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub node: String,
	#[field(primary_key = true, field_type = "text")]
	pub source_node: String,
	#[field(field_type = "text", null = true)]
	pub source_endpoint: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_dashboard_origin: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_public_key_pem: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_key_id: Option<String>,
	#[field]
	pub atomic_transactions: bool,
	#[field]
	pub enabled: bool,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub actor: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
}
