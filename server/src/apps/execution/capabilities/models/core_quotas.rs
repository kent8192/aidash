//! Persistent core_quotas records.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "core_quotas")]
#[derive(Serialize, Deserialize)]
pub struct CoreQuotas {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field]
	pub used_bytes: i64,
}
