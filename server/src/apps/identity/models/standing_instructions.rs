//! Tenant Instructions and Node Instructions (#1010): standing directions for
//! the Agents of a Tenant and of one Node.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "tenant_instructions")]
#[derive(Serialize, Deserialize)]
pub struct TenantInstructions {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub text: String,
	#[field]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub updated_by: String,
}

#[model(app_label = "identity", table_name = "node_instructions")]
#[derive(Serialize, Deserialize)]
pub struct NodeInstructions {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub node: String,
	#[field(field_type = "text")]
	pub text: String,
	#[field]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub updated_by: String,
}
