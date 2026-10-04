//! Persistent channel_attachments records.

use reinhardt::macros::Model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Model, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[model_config(app_label = "workspaces", table_name = "channel_attachments")]
pub struct ChannelAttachment {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,

	#[field(db_column = "workspace_id")]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub uploaded_by: String,
	#[field]
	pub idempotency_key: uuid::Uuid,
	#[field(field_type = "text")]
	pub filename: String,
	#[field(field_type = "text")]
	pub media_type: String,
	#[field(field_type = "text")]
	pub sha256: String,
	#[field]
	pub size_bytes: i64,
	#[field(default = 0)]
	pub position: i32,
	#[field]
	pub content: Vec<u8>,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(db_column = "message_id", null = true)]
	pub message_key: Option<uuid::Uuid>,
}

use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
impl ChannelAttachment {
	pub(crate) async fn has_media_in(
		tx: &mut dyn TransactionExecutor,
		message: Uuid,
	) -> Result<bool> {
		Ok(!Self::objects()
			.filter(Self::field_message_key().eq(Some(message)))
			.limit(1)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.is_empty())
	}
	pub(crate) async fn attach_run_media_in(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
		message: Uuid,
		principal: &str,
		ids: &[Uuid],
	) -> Result<Vec<Self>> {
		if ids.is_empty() || ids.len() > 8 {
			return Err(Error::Invalid("run media input exceeds count limit".into()));
		}
		if ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len() {
			return Err(Error::Invalid("attachment ids must be unique".into()));
		}
		let mut records = Self::objects()
			.filter(Self::field_workspace_id().eq(workspace))
			.filter(Self::field_uploaded_by().eq(principal.to_owned()))
			.filter(Self::field_id().is_in(ids.to_vec()))
			.order_by(&["id"])
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		if records.len() != ids.len() {
			return Err(Error::NotFound("attachment unavailable".into()));
		}
		if records
			.iter()
			.fold(0_i64, |sum, record| sum.saturating_add(record.size_bytes))
			> 8 * 1024 * 1024
		{
			return Err(Error::Invalid("run media input exceeds byte limit".into()));
		}
		if records
			.iter()
			.any(|record| record.message_key.is_some_and(|id| id != message))
		{
			return Err(Error::Conflict(
				"attachment is already linked to another message".into(),
			));
		}
		let saved = Self::objects()
			.filter(Self::field_workspace_id().eq(workspace))
			.filter(Self::field_message_key().eq(Some(message)))
			.order_by(&["position", "id"])
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		if !saved.is_empty() && saved.iter().map(|row| row.id).collect::<Vec<_>>() != ids {
			return Err(Error::Conflict(
				"run message attachment order changed".into(),
			));
		}
		for (position, id) in ids.iter().enumerate() {
			let (sql, values) = Query::update()
				.table(Alias::new("channel_attachments"))
				.value("message_id", message)
				.value(
					"position",
					i32::try_from(position)
						.map_err(|_| Error::Invalid("too many attachments".into()))?,
				)
				.and_where(Expr::col("id").eq(Expr::value(*id)))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.and_where(Expr::col("message_id").is_null())
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		let mut ordered = Vec::with_capacity(ids.len());
		for id in ids {
			let index = records
				.iter()
				.position(|row| row.id == *id)
				.expect("validated attachment id");
			ordered.push(records.remove(index));
		}
		Ok(ordered)
	}
}

impl crate::database::Record for ChannelAttachment {
	fn decode(row: &sqlx::postgres::PgRow) -> std::result::Result<Self, sqlx::Error> {
		use sqlx::Row;
		Ok(Self {
			id: row.try_get("id")?,
			workspace_id: row.try_get("workspace_id")?,
			uploaded_by: row.try_get("uploaded_by")?,
			idempotency_key: row.try_get("idempotency_key")?,
			filename: row.try_get("filename")?,
			media_type: row.try_get("media_type")?,
			sha256: row.try_get("sha256")?,
			size_bytes: row.try_get("size_bytes")?,
			position: row.try_get("position")?,
			content: row.try_get("content")?,
			message_key: row.try_get("message_id")?,
		})
	}
}
impl From<ChannelAttachment> for aidash_domain::workspaces::channels::AttachmentState {
	fn from(row: ChannelAttachment) -> Self {
		Self {
			id: row.id,
			workspace_id: row.workspace_id,
			uploaded_by: row.uploaded_by,
			idempotency_key: row.idempotency_key,
			filename: row.filename,
			media_type: row.media_type,
			sha256: row.sha256,
			size_bytes: row.size_bytes,
			content: row.content,
			message_id: row.message_key,
		}
	}
}
