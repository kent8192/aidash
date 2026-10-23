//! Persistent desktop_handoffs records.
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "identity", table_name = "desktop_handoffs")]
#[derive(Serialize, Deserialize)]
pub struct DesktopHandoff {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub state: String,
	#[field(field_type = "text")]
	pub challenge: String,
	#[field(field_type = "text")]
	pub redirect_uri: String,
	#[field(field_type = "text")]
	pub origin: String,
	#[rel(
		foreign_key,
		db_constraint = "desktop_handoffs_browser_session_id_fkey",
		db_index = false,
		null = true,
		on_delete = Cascade
	)]
	pub browser_session: ForeignKeyField<crate::apps::identity::models::DashboardSession>,
	#[field(null = true, unique = true)]
	pub code_hash: Option<Vec<u8>>,
	#[field]
	pub expires_at: chrono::DateTime<chrono::Utc>,
}
