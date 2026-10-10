//! Append-only record of who decided a Registration Request or disabled a Mapping.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(
	app_label = "identity",
	table_name = "dashboard_administration_history"
)]
#[derive(Serialize, Deserialize)]
pub struct DashboardAdministrationHistory {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub occurred_at: DateTime<Utc>,
	/// `registration.approve`, `registration.reject` or `mapping.disable`.
	#[field(field_type = "text")]
	pub action: String,
	#[field]
	pub identity_id: uuid::Uuid,
	#[field(null = true)]
	pub registration_id: Option<uuid::Uuid>,
	#[field(null = true)]
	pub mapping_id: Option<uuid::Uuid>,
	#[field(field_type = "text", null = true)]
	pub tenant: Option<String>,
	#[field(field_type = "text", null = true)]
	pub subject: Option<String>,
	/// `operator` or `tenant_administrator`.
	#[field(field_type = "text")]
	pub actor_kind: String,
	/// The acting External Identity; absent for the operator bearer token.
	#[field(null = true)]
	pub actor_identity_id: Option<uuid::Uuid>,
	/// The Mapping a Tenant Administrator acted through.
	#[field(null = true)]
	pub actor_mapping_id: Option<uuid::Uuid>,
}
