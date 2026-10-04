//! Persistent messages records.
use reinhardt::core::exception::Error as FrameworkError;

use crate::apps::execution::models::event_records;
use crate::apps::workspaces::serializers::entities::Message as MessageContract;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor, QueryRow};
use reinhardt::model;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, IntoIden, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[model(app_label = "workspaces", table_name = "messages")]
#[derive(Serialize, Deserialize)]
pub struct Message {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub sender: String,
	#[field(field_type = "text")]
	pub content: String,
	#[field(field_type = "text", null = true)]
	pub idempotency_key: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl Message {
	pub(crate) async fn append_in(
		tx: &mut dyn TransactionExecutor,
		node: &str,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<MessageContract> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new("messages"))
			.columns([
				Alias::new("id"),
				Alias::new("workspace_id"),
				Alias::new("sender"),
				Alias::new("content"),
				Alias::new("idempotency_key"),
			])
			.values_panic([
				IntoValue::into_value(Uuid::new_v4()),
				IntoValue::into_value(workspace),
				IntoValue::into_value(sender),
				IntoValue::into_value(content),
				IntoValue::into_value(key),
			])
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
			event_records::append(tx, node, Some(workspace), "message.created", json!(message))
				.await?;
			Ok(message)
		} else {
			let message = Self::objects()
				.filter(Self::field_idempotency_key().eq(Some(key.to_owned())))
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("message".into()))?;
			if message.workspace_id() != workspace
				|| message.sender != sender
				|| message.content != content
			{
				return Err(Error::Conflict("message idempotency key reused".into()));
			}
			Ok(message.into())
		}
	}
}

impl Message {
	pub(crate) async fn run_history<E: OrmExecutor>(
		db: &mut E,
		workspace: Uuid,
		peer: &str,
		task: Uuid,
		run: Uuid,
		offset: u64,
	) -> Result<Vec<MessageContract>> {
		let prefix = format!("{peer}:{task}:");
		let suffix = SimpleExpr::FunctionCall(
			"right".into_iden(),
			vec![
				Expr::col("idempotency_key").into(),
				Expr::value(74_i32).cast_as("int4"),
			],
		);
		let (sql, values) = Query::select().expr(Expr::asterisk()).from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Condition::any()
				.add(Expr::col("idempotency_key").starts_with(format!("{prefix}human:{run}:")))
				.add(Condition::all()
					.add(Expr::col("idempotency_key").starts_with(format!("{prefix}subject-human:")))
					.add(suffix.starts_with(format!(":{run}:")))))
			.order_by(Alias::new("created_at"), Order::Asc).order_by(Alias::new("id"), Order::Asc)
			// Four maximally escaped 64 KiB messages stay below the peer response cap.
			.limit(4).offset(offset).build(PostgresQueryBuilder);
		db.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				serde_json::from_value(QueryRow::from_backend_row(row).data).map_err(Error::from)
			})
			.collect()
	}
}

use reinhardt::query::IntoValue;

impl Message {}

impl crate::database::Record for Message {
	fn decode(row: &sqlx::postgres::PgRow) -> std::result::Result<Self, sqlx::Error> {
		use sqlx::Row;
		Ok(Self {
			id: row.try_get("id")?,
			workspace_id: row.try_get("workspace_id")?,
			sender: row.try_get("sender")?,
			content: row.try_get("content")?,
			idempotency_key: row.try_get("idempotency_key")?,
			created_at: row.try_get("created_at")?,
		})
	}
}
