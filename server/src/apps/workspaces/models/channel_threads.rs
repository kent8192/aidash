//! Persistent channel_threads records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "workspaces", table_name = "channel_threads")]
#[derive(Serialize, Deserialize)]
pub struct ChannelThread {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(db_column = "workspace_id")]
	pub workspace_key: uuid::Uuid,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(db_column = "root_message_id")]
	pub root_message_key: uuid::Uuid,
	#[field(field_type = "text")]
	pub created_by: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl crate::database::Record for ChannelThread {
	fn decode(row: &crate::database::native::Row) -> crate::Result<Self> {
		Ok(Self {
			id: row.try_get("id")?,
			workspace_key: row.try_get("workspace_id")?,
			root_message_key: row.try_get("root_message_id")?,
			created_by: row.try_get("created_by")?,
			created_at: row.try_get("created_at")?,
		})
	}
}
impl From<ChannelThread> for aidash_domain::workspaces::channels::ChannelThread {
	fn from(row: ChannelThread) -> Self {
		Self {
			id: row.id,
			workspace_id: row.workspace_key,
			root_message_id: row.root_message_key,
			created_by: row.created_by,
			created_at: row.created_at,
		}
	}
}
