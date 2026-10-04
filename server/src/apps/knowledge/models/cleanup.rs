//! Lock cleanup identities through external deletion and its acknowledgement.
use super::{SemanticCollection, SemanticPoint, index_configuration::clock};
use crate::Result;
use crate::apps::knowledge::serializers::contracts::{CleanupCounts, CleanupStatus, VectorConfig};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::{DatabaseConnection, TransactionExecutor};
use reinhardt::db::orm::{Model, QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, Func, JoinType, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, TableRef,
};
use uuid::Uuid;

impl SemanticPoint {
	pub(crate) async fn cleanup_due(db: &DatabaseConnection) -> Result<Vec<Uuid>> {
		let (sql, values) = Query::select()
			.column(("p", "id"))
			.from_as(Alias::new(Self::table_name()), Alias::new("p"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(
					Alias::new(SemanticCollection::table_name()),
					Alias::new("c"),
				),
				reinhardt::query::SimpleExpr::from(Expr::col(("c", "collection")))
					.eq(Expr::col(("p", "collection"))),
			)
			.and_where(Expr::col(("p", "retired")).eq(reinhardt::query::Expr::value(true)))
			.and_where(Expr::col(("p", "cleaned_at")).is_null())
			.and_where(Expr::col(("c", "retired")).eq(reinhardt::query::Expr::value(false)))
			.and_where(Expr::col(("p", "next_attempt")).lte(clock()))
			.order_by(("p", "next_attempt"), Order::Asc)
			.order_by(("p", "id"), Order::Asc)
			.limit(32)
			.build(PostgresQueryBuilder);
		db.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				row.get("id")
					.map_err(FrameworkError::from)
					.map_err(Into::into)
			})
			.collect()
	}

	pub(crate) async fn lock_cleanup(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
	) -> Result<Option<(String, VectorConfig)>> {
		let (sql, values) = Query::select()
			.column(("p", "collection"))
			.column(("c", "vector"))
			.from_as(Alias::new(Self::table_name()), Alias::new("p"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(
					Alias::new(SemanticCollection::table_name()),
					Alias::new("c"),
				),
				reinhardt::query::SimpleExpr::from(Expr::col(("c", "collection")))
					.eq(Expr::col(("p", "collection"))),
			)
			.and_where(Expr::col(("p", "id")).eq(Expr::value(id)))
			.and_where(Expr::col(("p", "retired")).eq(reinhardt::query::Expr::value(true)))
			.and_where(Expr::col(("p", "cleaned_at")).is_null())
			.and_where(Expr::col(("p", "next_attempt")).lte(clock()))
			.lock(LockType::Update)
			.lock_tables([Alias::new("p")])
			.lock_behavior(LockBehavior::SkipLocked)
			.build(PostgresQueryBuilder);
		tx.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| {
				let row = QueryRow::from_backend_row(row).data;
				Ok((
					serde_json::from_value(row["collection"].clone())?,
					serde_json::from_value(row["vector"].clone())?,
				))
			})
			.transpose()
	}

	pub(crate) async fn record_cleanup(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		failure: Option<&str>,
	) -> Result<()> {
		let mut query = Query::update();
		query
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("last_error"), Expr::value(failure))
			.value_expr(
				Alias::new("next_attempt"),
				clock().add(Expr::value("30 seconds").cast_as("interval")),
			)
			.and_where(Expr::col("id").eq(Expr::value(id)));
		if failure.is_none() {
			query.value_expr(Alias::new("cleaned_at"), clock());
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn cleanup_counts(
		db: &DatabaseConnection,
		workspace: Uuid,
	) -> Result<CleanupCounts> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::count(Expr::col(ColumnRef::Asterisk).into()),
				Alias::new("retired"),
			)
			.expr_as(
				Func::count(
					Expr::case()
						.when(Expr::col(("p", "cleaned_at")).is_null(), Expr::value(1_i64))
						.else_result(Expr::null())
						.into(),
				),
				Alias::new("pending"),
			)
			.expr_as(
				Func::count(
					Expr::case()
						.when(
							Expr::col(("p", "last_error")).is_not_null(),
							Expr::value(1_i64),
						)
						.else_result(Expr::null())
						.into(),
				),
				Alias::new("failed"),
			)
			.from_as(Alias::new(Self::table_name()), Alias::new("p"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(
					Alias::new(SemanticCollection::table_name()),
					Alias::new("c"),
				),
				reinhardt::query::SimpleExpr::from(Expr::col(("c", "collection")))
					.eq(Expr::col(("p", "collection"))),
			)
			.and_where(Expr::col(("c", "workspace_id")).eq(Expr::value(workspace)))
			.and_where(Expr::col(("p", "retired")).eq(reinhardt::query::Expr::value(true)))
			.and_where(Expr::col(("c", "retired")).eq(reinhardt::query::Expr::value(false)))
			.build(PostgresQueryBuilder);
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(db.fetch_one(&sql, convert_values(values)).await?).data,
		)?)
	}
}

impl SemanticCollection {
	pub(crate) async fn cleanup_due(db: &DatabaseConnection) -> Result<Vec<String>> {
		let (sql, values) = Query::select()
			.column(Alias::new("collection"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("retired").eq(reinhardt::query::Expr::value(true)))
			.and_where(Expr::col("cleaned_at").is_null())
			.and_where(Expr::col("next_attempt").lte(clock()))
			.order_by(Alias::new("next_attempt"), Order::Asc)
			.order_by(Alias::new("collection"), Order::Asc)
			.limit(16)
			.build(PostgresQueryBuilder);
		db.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				row.get("collection")
					.map_err(FrameworkError::from)
					.map_err(Into::into)
			})
			.collect()
	}

	pub(crate) async fn lock_cleanup(
		tx: &mut dyn TransactionExecutor,
		collection: &str,
	) -> Result<Option<VectorConfig>> {
		let (sql, values) = Query::select()
			.column(Alias::new("vector"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("collection").eq(Expr::value(collection)))
			.and_where(Expr::col("retired").eq(reinhardt::query::Expr::value(true)))
			.and_where(Expr::col("cleaned_at").is_null())
			.and_where(Expr::col("next_attempt").lte(clock()))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| {
				serde_json::from_value(QueryRow::from_backend_row(row).data["vector"].clone())
			})
			.transpose()?)
	}

	pub(crate) async fn record_cleanup(
		tx: &mut dyn TransactionExecutor,
		collection: &str,
		failure: Option<&str>,
	) -> Result<()> {
		let mut query = Query::update();
		query
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("last_error"), Expr::value(failure))
			.value_expr(
				Alias::new("next_attempt"),
				clock().add(Expr::value("30 seconds").cast_as("interval")),
			)
			.and_where(Expr::col("collection").eq(Expr::value(collection)));
		if failure.is_none() {
			query.value_expr(Alias::new("cleaned_at"), clock());
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn cleanup_counts(
		db: &DatabaseConnection,
		workspace: Uuid,
	) -> Result<CleanupCounts> {
		let (sql, values) = Query::select()
			.expr_as(
				Func::count(Expr::col(ColumnRef::Asterisk).into()),
				Alias::new("retired"),
			)
			.expr_as(
				Func::count(
					Expr::case()
						.when(Expr::col("cleaned_at").is_null(), Expr::value(1_i64))
						.else_result(Expr::null())
						.into(),
				),
				Alias::new("pending"),
			)
			.expr_as(
				Func::count(
					Expr::case()
						.when(Expr::col("last_error").is_not_null(), Expr::value(1_i64))
						.else_result(Expr::null())
						.into(),
				),
				Alias::new("failed"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("retired").eq(reinhardt::query::Expr::value(true)))
			.build(PostgresQueryBuilder);
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(db.fetch_one(&sql, convert_values(values)).await?).data,
		)?)
	}
}

pub(crate) async fn status(db: &DatabaseConnection, workspace: Uuid) -> Result<CleanupStatus> {
	Ok(CleanupStatus {
		points: SemanticPoint::cleanup_counts(db, workspace).await?,
		collections: SemanticCollection::cleanup_counts(db, workspace).await?,
	})
}
