//! Persistent desktop_refresh_credentials records.
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "identity", table_name = "desktop_refresh_credentials")]
#[derive(Serialize, Deserialize)]
pub struct DesktopRefreshCredential {
	#[field(primary_key = true)]
	pub token_hash: super::byte_key::ByteKey,
	#[rel(
		foreign_key,
		db_constraint = "desktop_refresh_credentials_session_id_fkey",
		db_index = false,
		on_delete = Cascade
	)]
	pub session: ForeignKeyField<crate::apps::identity::models::DashboardSession>,
	#[field(null = true)]
	pub next_hash: Option<Vec<u8>>,
}
