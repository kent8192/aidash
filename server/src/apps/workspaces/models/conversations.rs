//! Persistent conversations records.

use crate::apps::workspaces::services::states::ConversationTargetKind;
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "workspaces", table_name = "conversations")]
#[derive(Serialize, Deserialize)]
pub struct Conversation {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub target: String,
	#[field(field_type = "text", max_length = 64)]
	pub target_kind: ConversationTargetKind,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(field_type = "text")]
	pub created_by: String,
}

impl Conversation {}

impl crate::database::Record for Conversation {
	fn decode(row: &crate::database::native::Row) -> crate::Result<Self> {
		let kind: String = row.try_get("target_kind")?;
		let target_kind = serde_json::from_value(serde_json::Value::String(kind))?;
		Ok(Self {
			id: row.try_get("id")?,
			workspace_id: row.try_get("workspace_id")?,
			target: row.try_get("target")?,
			target_kind,
			created_at: row.try_get("created_at")?,
			created_by: row.try_get("created_by")?,
		})
	}
}
impl From<Conversation> for aidash_domain::Conversation {
	fn from(record: Conversation) -> Self {
		Self {
			id: record.id,
			workspace_id: record.workspace_id,
			target: record.target,
			target_kind: match record.target_kind {
				ConversationTargetKind::Agent => "agent",
				ConversationTargetKind::Cluster => "cluster",
			}
			.into(),
			created_at: record.created_at,
			created_by: record.created_by,
		}
	}
}
