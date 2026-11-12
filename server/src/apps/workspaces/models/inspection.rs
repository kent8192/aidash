//! Transaction-bound records used by authorization and factual inspection.
use super::{Artifact, Conversation, Message, Task, Workspace};
use crate::domain;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, IntoIden, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

impl Task {
	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<domain::Task>> {
		Ok(Self::objects()
			.filter(Self::field_id().eq(id))
			.filter(Self::field_workspace_id().eq(workspace))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(Into::into))
	}

	/// A walk over recorded reads loads them together rather than one at a time.
	pub(crate) async fn read_many_in(
		tx: &mut dyn TransactionExecutor,
		ids: &[Uuid],
	) -> Result<Vec<domain::Task>> {
		Ok(rows_by_id::<Self>(tx, Self::table_name(), ids)
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
}

async fn rows_by_id<T: serde::de::DeserializeOwned>(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	ids: &[Uuid],
) -> Result<Vec<T>> {
	let (sql, values) = Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new(table))
		.and_where(Expr::col("id").is_in(ids.iter().copied().map(Expr::value)))
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data).map_err(Into::into))
		.collect()
}

impl Artifact {
	/// See [`Task::read_many_in`].
	pub(crate) async fn read_many_in(
		tx: &mut dyn TransactionExecutor,
		ids: &[Uuid],
	) -> Result<Vec<domain::Artifact>> {
		rows_by_id(tx, Self::table_name(), ids).await
	}

	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		workspace: Uuid,
		lock: bool,
	) -> Result<Option<domain::Artifact>> {
		let mut query = Query::select();
		query
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)));
		if lock {
			query.lock(LockType::Share);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.transpose()?)
	}
}

impl Message {
	/// See [`Task::read_many_in`].
	pub(crate) async fn read_many_in(
		tx: &mut dyn TransactionExecutor,
		ids: &[Uuid],
	) -> Result<Vec<domain::Message>> {
		rows_by_id(tx, Self::table_name(), ids).await
	}

	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		workspace: Uuid,
		lock: bool,
	) -> Result<Option<domain::Message>> {
		let mut query = Query::select();
		query
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)));
		if lock {
			query.lock(LockType::Share);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.transpose()?)
	}
}

impl Conversation {
	/// See [`Task::read_many_in`].
	pub(crate) async fn read_many_in(
		tx: &mut dyn TransactionExecutor,
		ids: &[Uuid],
	) -> Result<Vec<domain::Conversation>> {
		rows_by_id(tx, Self::table_name(), ids).await
	}

	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<domain::Conversation>> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.transpose()?)
	}
}

impl Workspace {
	pub(crate) async fn snapshot_rows<E: OrmExecutor>(
		db: &mut E,
		workspace: Uuid,
		collection: &str,
		after: Option<Uuid>,
	) -> Result<Vec<(Uuid, Value)>> {
		if !matches!(collection, "tasks" | "artifacts" | "events" | "messages") {
			return Err(Error::Invalid("unknown snapshot collection".into()));
		}
		let mut source = Query::select();
		source
			.column(ColumnRef::Asterisk)
			.from(Alias::new(collection))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)));
		if collection == "events" {
			source
				.order_by(Alias::new("sequence"), Order::Desc)
				.limit(100);
		} else if collection == "messages" {
			source
				.order_by(Alias::new("created_at"), Order::Desc)
				.order_by(Alias::new("id"), Order::Desc)
				.limit(100);
		}
		let mut query = Query::select();
		query
			.column((Alias::new("item"), Alias::new("id")))
			.expr_as(
				SimpleExpr::FunctionCall("to_jsonb".into_iden(), vec![Expr::col("item").into()]),
				Alias::new("record"),
			)
			.from_subquery(source, Alias::new("item"))
			.order_by((Alias::new("item"), Alias::new("id")), Order::Asc)
			.limit(32);
		if let Some(after) = after {
			query.and_where(
				Expr::col((Alias::new("item"), Alias::new("id"))).gt(Expr::value(after)),
			);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		db.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				let id = row.get("id").map_err(FrameworkError::from)?;
				Ok((id, QueryRow::from_backend_row(row).data["record"].clone()))
			})
			.collect()
	}

	pub(crate) async fn title_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
	) -> Result<Option<String>> {
		Ok(Self::objects()
			.filter(Self::field_id().eq(id))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|row| row.title))
	}
}
