//! Persistent channel_attachments records.

use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "workspaces", table_name = "channel_attachments")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelAttachment {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,

	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "channel_attachments_thread_id_fkey",
		db_index = false,
		on_delete = Cascade
	)]
	pub thread: ForeignKeyField<crate::apps::workspaces::models::Thread>,
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
	#[field(null = true)]
	pub content: Option<Vec<u8>>,
	#[field(null = true)]
	pub object_id: Option<uuid::Uuid>,
	#[field(null = true)]
	pub width: Option<i32>,
	#[field(null = true)]
	pub height: Option<i32>,
	#[field(null = true)]
	pub duration_ms: Option<i64>,
	#[field(null = true)]
	pub wav_object_id: Option<uuid::Uuid>,
	#[field(field_type = "text", null = true)]
	pub wav_sha256: Option<String>,
	#[field(null = true)]
	pub wav_size_bytes: Option<i64>,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(db_column = "message_id", null = true)]
	pub message_key: Option<uuid::Uuid>,
	#[field(null = true)]
	pub planned_completion: Option<uuid::Uuid>,
	/// The Image Description its uploader wrote for an image (#622).
	#[field(field_type = "text", null = true)]
	pub description: Option<String>,
}

use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
pub(crate) struct CompletionAttachment<'a> {
	pub id: Uuid,
	pub thread: Uuid,
	pub sender: &'a str,
	pub message: Uuid,
	pub filename: &'a str,
	pub media_type: &'a str,
	pub digest: &'a str,
	pub size: u64,
	pub object: Uuid,
}

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
	pub(crate) async fn stage_completion(
		tx: &mut dyn TransactionExecutor,
		input: CompletionAttachment<'_>,
	) -> Result<()> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new("channel_attachments"))
			.columns(
				[
					"id",
					"thread_id",
					"uploaded_by",
					"idempotency_key",
					"filename",
					"media_type",
					"sha256",
					"size_bytes",
					"object_id",
					"tenant",
					"planned_completion",
				]
				.map(Alias::new),
			)
			.values_expr(vec![
				Expr::value(input.id).into(),
				Expr::value(input.thread).into(),
				Expr::value(input.sender).into(),
				Expr::value(input.id).into(),
				Expr::value(input.filename).into(),
				Expr::value(input.media_type).into(),
				Expr::value(input.digest).into(),
				Expr::value(input.size as i64).into(),
				Expr::value(input.object).into(),
				crate::database::scope::tenant_of("threads", "id", input.thread),
				Expr::value(input.message).into(),
			])
			.expect("one value per column")
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn attach_completion(
		tx: &mut dyn TransactionExecutor,
		thread: Uuid,
		message: Uuid,
		sender: &str,
		ids: &[Uuid],
	) -> Result<()> {
		use aidash_domain::deliverable::{
			MAX_COMPLETION_ATTACHMENT_BYTES, MAX_COMPLETION_ATTACHMENTS,
		};
		if ids.is_empty() {
			return Ok(());
		}
		if ids.len() > MAX_COMPLETION_ATTACHMENTS
			|| ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
		{
			return Err(Error::Invalid(
				"Invalid Completion Message attachment count".into(),
			));
		}
		let rows = Self::objects()
			.filter(Self::field_thread_id().eq(thread))
			.filter(Self::field_uploaded_by().eq(sender.to_owned()))
			.filter(Self::field_planned_completion().eq(Some(message)))
			.filter(Self::field_id().is_in(ids.to_vec()))
			.order_by(&["id"])
			.select_for_update()
			.all_with_executor(&mut *tx)
			.await
			.map_err(FrameworkError::from)?;
		if rows.len() != ids.len()
			|| rows
				.iter()
				.any(|record| record.message_key.is_some_and(|id| id != message))
		{
			return Err(Error::Invalid(
				"Completion Message attachments are unavailable".into(),
			));
		}
		let bytes = rows
			.iter()
			.fold(0u64, |sum, row| sum.saturating_add(row.size_bytes as u64));
		if bytes > MAX_COMPLETION_ATTACHMENT_BYTES {
			return Err(Error::Invalid(
				"Completion Message attachments exceed 4 GiB".into(),
			));
		}
		for (position, id) in ids.iter().enumerate() {
			let (sql, values) = Query::update()
				.table(Alias::new("channel_attachments"))
				.value_expr(Alias::new("message_id"), Expr::value(message))
				.value_expr(Alias::new("position"), Expr::value(position as i32))
				.and_where(Expr::col("id").eq(Expr::value(*id)))
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		Ok(())
	}

	pub(crate) async fn attach_run_media_in(
		tx: &mut dyn TransactionExecutor,
		thread: Uuid,
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
			.filter(Self::field_thread_id().eq(thread))
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
			.filter(Self::field_thread_id().eq(thread))
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
				.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
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
	fn decode(row: &crate::database::native::Row) -> crate::Result<Self> {
		Ok(Self {
			id: row.try_get("id")?,
			tenant: row.try_get("tenant")?,
			thread_id: row.try_get("thread_id")?,
			thread: ForeignKeyField::new(),
			uploaded_by: row.try_get("uploaded_by")?,
			idempotency_key: row.try_get("idempotency_key")?,
			filename: row.try_get("filename")?,
			media_type: row.try_get("media_type")?,
			sha256: row.try_get("sha256")?,
			size_bytes: row.try_get("size_bytes")?,
			position: row.try_get("position")?,
			content: row.try_get("content")?,
			object_id: row.try_get("object_id")?,
			width: row.try_get("width")?,
			height: row.try_get("height")?,
			duration_ms: row.try_get("duration_ms")?,
			wav_object_id: row.try_get("wav_object_id")?,
			wav_sha256: row.try_get("wav_sha256")?,
			wav_size_bytes: row.try_get("wav_size_bytes")?,
			message_key: row.try_get("message_id")?,
			planned_completion: row.try_get("planned_completion")?,
			description: row.try_get("description")?,
		})
	}
}
impl From<ChannelAttachment> for aidash_domain::threads::channels::AttachmentState {
	fn from(row: ChannelAttachment) -> Self {
		Self {
			id: row.id,
			thread_id: row.thread_id,
			uploaded_by: row.uploaded_by,
			idempotency_key: row.idempotency_key,
			filename: row.filename,
			media_type: row.media_type,
			sha256: row.sha256,
			size_bytes: row.size_bytes,
			content: row.content,
			object_id: row.object_id,
			message_id: row.message_key,
			description: row.description,
		}
	}
}
