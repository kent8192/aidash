//! Persistent authorization_catalog records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_catalog")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationCatalog {
	#[field(field_type = "text", primary_key = true)]
	pub tenant: String,
	/// The Node the approval applies to.
	#[field(primary_key = true, field_type = "text")]
	pub node: String,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, field_type = "text", db_column = "entry_id")]
	pub entry_key: String,
	#[field(primary_key = true, field_type = "text")]
	pub entry_version: String,
	#[field]
	pub enabled: bool,
	#[field]
	pub revision: i64,
	/// Subjects and Assignable Groups that may use the definition on this
	/// Node; NULL means everyone the Tenant's policy allows (ADR 0095).
	#[field(null = true)]
	pub restriction: Option<reinhardt::db::orm::Json<serde_json::Value>>,
}

impl AuthorizationCatalog {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
