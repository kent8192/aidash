//! Persistent Marketplace identities and immutable document revisions.
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "marketplace", table_name = "marketplace_gate")]
#[derive(Serialize, Deserialize)]
pub struct CompatibilityGate {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
}

#[model(app_label = "marketplace", table_name = "marketplace_versions")]
#[derive(Serialize, Deserialize)]
pub struct PublishedVersion {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
	#[field(field_type = "text")]
	pub repository: String,
	#[field(field_type = "text")]
	pub owner: String,
	#[field(field_type = "text")]
	pub package_id: String,
	#[field(field_type = "text")]
	pub version: String,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(field_type = "text")]
	pub source_id: String,
	#[field(field_type = "text")]
	pub source_version: String,
	#[field(field_type = "text")]
	pub source_content: String,
}

#[model(app_label = "marketplace", table_name = "marketplace_audiences")]
#[derive(Serialize, Deserialize)]
pub struct DistributionAudience {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
}

#[model(app_label = "marketplace", table_name = "marketplace_consents")]
#[derive(Serialize, Deserialize)]
pub struct RedistributionConsent {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
}

#[model(app_label = "marketplace", table_name = "marketplace_installations")]
#[derive(Serialize, Deserialize)]
pub struct InstalledPackage {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub package_key: String,
}

#[model(app_label = "marketplace", table_name = "marketplace_revisions")]
#[derive(Serialize, Deserialize)]
pub struct InstalledRevision {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
	#[field(field_type = "text")]
	pub entry_id: String,
	#[field(field_type = "text")]
	pub entry_version: String,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub installation: String,
}

#[model(app_label = "marketplace", table_name = "marketplace_requests")]
#[derive(Serialize, Deserialize)]
pub struct IdempotentRequest {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
}

#[model(app_label = "marketplace", table_name = "marketplace_provenance")]
#[derive(Serialize, Deserialize)]
pub struct Provenance {
	#[field(primary_key = true, field_type = "text")]
	pub key: String,
	#[field]
	pub document: Json<Value>,
}

impl InstalledRevision {
	pub fn installed_id(&self) -> String {
		self.installation.clone()
	}
}
