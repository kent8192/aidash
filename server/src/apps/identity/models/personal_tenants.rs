//! The link from an External Identity to the Personal Tenant Aidash made for it.
//! It is written once; sign-in provisions a Personal Tenant only when none
//! exists, so a withdrawn Mapping or policy is never repaired.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "personal_tenants")]
#[derive(Serialize, Deserialize)]
pub struct PersonalTenantLink {
	#[field(primary_key = true)]
	pub identity_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub created_at: DateTime<Utc>,
}
