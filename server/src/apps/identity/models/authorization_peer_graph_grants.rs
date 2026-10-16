//! Persistent authorization_graph_operator_grants records.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(
	app_label = "identity",
	table_name = "authorization_graph_operator_grants"
)]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationGraphOperatorGrants {
	#[field(primary_key = true, field_type = "text")]
	pub source_node: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub source_operator: uuid::Uuid,
	#[field(field_type = "text", primary_key = true)]
	pub tenant: String,
	#[field]
	pub enabled: bool,
	#[field]
	pub revision: i64,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}

impl AuthorizationGraphOperatorGrants {
	pub fn bundle_id(&self) -> String {
		self.tenant.clone()
	}
}
