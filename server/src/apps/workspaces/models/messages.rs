//! Persistent messages records.
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::associations::ForeignKeyField;

use crate::apps::execution::models::event_records;
use crate::apps::workspaces::serializers::entities::Message as MessageContract;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[model(app_label = "workspaces", table_name = "messages")]
#[derive(Serialize, Deserialize)]
pub struct Message {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "messages_thread_id_fkey",
		db_index = false
	)]
	pub thread: ForeignKeyField<crate::apps::workspaces::models::Thread>,
	#[field(field_type = "text")]
	pub sender: String,
	#[field(field_type = "text")]
	pub content: String,
	#[field(field_type = "text", null = true)]
	pub idempotency_key: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	/// The Run that published this message on this Node; see the contract.
	#[field(null = true)]
	pub run_id: Option<uuid::Uuid>,
	#[field(null = true)]
	pub completion: Option<reinhardt::db::orm::Json<serde_json::Value>>,
}

impl Message {
	pub(crate) async fn append_completion(
		tx: &mut dyn TransactionExecutor,
		task: &aidash_domain::Task,
		sender: &str,
		key: &str,
		run: Option<Uuid>,
		completion: &aidash_domain::deliverable::Completion,
	) -> Result<MessageContract> {
		let details = aidash_domain::deliverable::CompletionDetails {
			task_id: task.id,
			verdict: completion.verdict.clone(),
			attachments: completion.attachments.clone(),
			deliverable: completion.deliverable.clone(),
		};
		let (sql, values) = Query::insert()
			.into_table(Alias::new("messages"))
			.columns(
				[
					"id",
					"thread_id",
					"sender",
					"content",
					"idempotency_key",
					"run_id",
					"tenant",
					"completion",
				]
				.map(Alias::new),
			)
			.values_expr(vec![
				Expr::value(completion.message_id).into(),
				Expr::value(task.thread_id).into(),
				Expr::value(sender).into(),
				Expr::value(&completion.text).into(),
				Expr::value(key).into(),
				Expr::value(run).into(),
				crate::database::scope::tenant_of("threads", "id", task.thread_id),
				Expr::value(serde_json::to_value(details)?).into(),
			])
			.expect("one value per column")
			.returning_all()
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		let message: MessageContract =
			serde_json::from_value(QueryRow::from_backend_row(row).data)?;
		super::ChannelAttachment::attach_completion(
			tx,
			task.thread_id,
			message.id,
			sender,
			&completion.attachments,
		)
		.await?;
		event_records::append(
			tx,
			Some(task.thread_id),
			"message.created",
			json!({"id": message.id, "thread_id": task.thread_id}),
		)
		.await?;
		Ok(message)
	}

	pub(crate) async fn append_in(
		tx: &mut dyn TransactionExecutor,
		thread: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		run: Option<Uuid>,
	) -> Result<MessageContract> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new("messages"))
			.columns([
				Alias::new("id"),
				Alias::new("thread_id"),
				Alias::new("sender"),
				Alias::new("content"),
				Alias::new("idempotency_key"),
				Alias::new("run_id"),
				Alias::new("tenant"),
			])
			.values_expr(vec![
				Expr::value(Uuid::new_v4()).into(),
				Expr::value(thread).into(),
				Expr::value(sender).into(),
				Expr::value(content).into(),
				Expr::value(key).into(),
				Expr::value(run).into(),
				crate::database::scope::tenant_of("threads", "id", thread),
			])
			.expect("one value per column")
			.on_conflict(
				OnConflict::columns([Alias::new("idempotency_key")])
					.do_nothing()
					.to_owned(),
			)
			.returning_all()
			.build(PostgresQueryBuilder);
		if let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? {
			let message: MessageContract =
				serde_json::from_value(QueryRow::from_backend_row(row).data)?;
			event_records::append(tx, Some(thread), "message.created", json!(message)).await?;
			Ok(message)
		} else {
			let message = Self::objects()
				.filter(Self::field_idempotency_key().eq(Some(key.to_owned())))
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("message".into()))?;
			// The publishing Run is part of the input: a replay that would bind
			// another Run's citations, or none, is a reused key.
			if message.thread_id() != thread
				|| message.sender != sender
				|| message.content != content
				|| message.run_id != run
			{
				return Err(Error::Conflict("message idempotency key reused".into()));
			}
			Ok(message.into())
		}
	}
}

impl Message {}

impl Message {}

impl crate::database::Record for Message {
	fn decode(row: &crate::database::native::Row) -> crate::Result<Self> {
		Ok(Self {
			id: row.try_get("id")?,
			tenant: row.try_get("tenant")?,
			thread_id: row.try_get("thread_id")?,
			thread: ForeignKeyField::new(),
			sender: row.try_get("sender")?,
			content: row.try_get("content")?,
			completion: row
				.try_get::<Option<serde_json::Value>>("completion")?
				.map(Into::into),
			idempotency_key: row.try_get("idempotency_key")?,
			created_at: row.try_get("created_at")?,
			run_id: row.try_get("run_id")?,
		})
	}
}
