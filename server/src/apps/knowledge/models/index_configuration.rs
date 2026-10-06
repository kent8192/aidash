//! Immutable index generations and their durable physical cleanup identities.
use super::{SemanticCollection, SemanticEntry, SemanticHistory, SemanticIndexe, SemanticPoint};
use crate::Result;
use crate::apps::knowledge::serializers::contracts::{Entry, Index};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, Func, IntoIden, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(super) fn clock() -> SimpleExpr {
	SimpleExpr::FunctionCall("clock_timestamp".into_iden(), vec![])
}

impl SemanticIndexe {
	pub(crate) async fn locked(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
		exclusive: bool,
	) -> Result<Option<Index>> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.lock(if exclusive {
				LockType::Update
			} else {
				LockType::Share
			})
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.transpose()?)
	}

	pub(crate) async fn replace_generation(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
		tenant: &str,
		revision: i64,
		spec: Value,
		collection: &str,
	) -> Result<Index> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(["workspace_id", "tenant", "revision", "spec", "collection"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(workspace),
				IntoValue::into_value(tenant),
				IntoValue::into_value(revision),
				IntoValue::into_value(spec),
				IntoValue::into_value(collection),
			])
			.on_conflict(OnConflict::columns(["workspace_id"]).update_columns([
				"revision",
				"spec",
				"collection",
				"updated_at",
			]))
			.returning_all()
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(row).data,
		)?)
	}
}

impl SemanticEntry {
	pub(crate) async fn configuration_counts(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
	) -> Result<(i64, usize)> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::count(Expr::col(ColumnRef::Asterisk).into()),
				Alias::new("count"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("deleted").eq(reinhardt::query::Expr::value(false)))
			.build(PostgresQueryBuilder);
		let count = tx
			.fetch_one(&sql, convert_values(values))
			.await?
			.get("count")
			.map_err(FrameworkError::from)?;
		let text = SimpleExpr::FunctionCall(
			"jsonb_extract_path_text".into_iden(),
			vec![Expr::col("source").into(), Expr::value("text").into()],
		);
		let kind = SimpleExpr::FunctionCall(
			"jsonb_extract_path_text".into_iden(),
			vec![Expr::col("source").into(), Expr::value("kind").into()],
		);
		let bytes = SimpleExpr::FunctionCall("octet_length".into_iden(), vec![text]);
		let (sql, values) = Query::select()
			.expr_as(
				Func::coalesce(vec![Func::max(bytes), Expr::value(0_i64).into()]),
				Alias::new("largest"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(kind.eq(Expr::value("memory")))
			.build(PostgresQueryBuilder);
		let largest: i64 = tx
			.fetch_one(&sql, convert_values(values))
			.await?
			.get("largest")
			.map_err(FrameworkError::from)?;
		Ok((count, largest as usize))
	}

	pub(crate) async fn lock_active(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
	) -> Result<Vec<Entry>> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("deleted").eq(reinhardt::query::Expr::value(false)))
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.collect::<std::result::Result<_, _>>()?)
	}

	pub(crate) async fn reconfigure(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		revision: i64,
	) -> Result<Entry> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("point_id"), Expr::value(Uuid::new_v4()))
			.value_expr(Alias::new("index_revision"), Expr::value(revision))
			.value_expr(Alias::new("state"), Expr::value("PENDING"))
			.value_expr(Alias::new("last_error"), Expr::null())
			.value_expr(Alias::new("attempts"), Expr::value(0_i32))
			.value_expr(Alias::new("next_attempt"), clock())
			.value_expr(Alias::new("updated_at"), clock())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.returning_all()
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(row).data,
		)?)
	}
}

impl SemanticCollection {
	pub(crate) async fn retire_workspace(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("retired"), Expr::value(true))
			.value_expr(Alias::new("next_attempt"), clock())
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn record_generation(
		tx: &mut dyn TransactionExecutor,
		collection: &str,
		workspace: Uuid,
		vector: Value,
	) -> Result<()> {
		let record = Self::build()
			.collection(collection)
			.workspace_id(workspace)
			.vector(vector.into())
			.retired(false)
			.last_error(None)
			.cleaned_at(None)
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}
}

impl SemanticPoint {
	pub(crate) async fn schedule(
		tx: &mut dyn TransactionExecutor,
		entry: &Entry,
		collection: &str,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("retired"), Expr::value(true))
			.value_expr(Alias::new("next_attempt"), clock())
			.and_where(Expr::col("entry_id").eq(Expr::value(entry.id)))
			.and_where(Expr::col("retired").eq(reinhardt::query::Expr::value(false)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		if !entry.deleted {
			let record = Self::build()
				.id(entry.point_id)
				.entry_id(entry.id)
				.collection(collection)
				.content_digest(None)
				.retired(false)
				.last_error(None)
				.cleaned_at(None)
				.finish();
			Self::objects()
				.insert_with_executor(tx, &record)
				.await
				.map_err(FrameworkError::from)?;
		}
		Ok(())
	}
}

impl SemanticHistory {
	pub(crate) async fn append(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
		entry: Option<Uuid>,
		revision: i64,
		state: &str,
		detail: &str,
	) -> Result<()> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(["workspace_id", "entry_id", "revision", "state", "detail"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(workspace),
				IntoValue::into_value(entry),
				IntoValue::into_value(revision),
				IntoValue::into_value(state),
				IntoValue::into_value(detail),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

use reinhardt::query::IntoValue;
