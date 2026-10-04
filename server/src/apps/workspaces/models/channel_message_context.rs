//! Persistent channel_message_context records.

use reinhardt::macros::Model;
use serde::{Deserialize, Serialize};

#[derive(Model, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[model_config(app_label = "workspaces", table_name = "channel_message_context")]
pub struct ChannelMessageContext {
	#[field(primary_key = true)]
	pub message_id: uuid::Uuid,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(db_column = "workspace_id")]
	pub workspace_key: uuid::Uuid,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(db_column = "thread_id", null = true)]
	pub thread_key: Option<uuid::Uuid>,
	#[field(field_type = "text")]
	pub attachment_digest: String,
}

impl crate::database::Record for ChannelMessageContext {
	fn decode(row: &sqlx::postgres::PgRow) -> std::result::Result<Self, sqlx::Error> {
		use sqlx::Row;
		Ok(Self {
			message_id: row.try_get("message_id")?,
			workspace_key: row.try_get("workspace_id")?,
			thread_key: row.try_get("thread_id")?,
			attachment_digest: row.try_get("attachment_digest")?,
		})
	}
}
