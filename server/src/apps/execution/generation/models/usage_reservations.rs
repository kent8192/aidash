//! Commit provider charges before external effects, and settle bounded usage atomically.
use super::{
	GenerationBudget, GenerationEmbeddingUsage, GenerationPolicy, GenerationRequest,
	GenerationUsage,
};
use crate::apps::registry::models::Definition;
use crate::registry::EntityRef;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

impl GenerationBudget {
	pub(crate) async fn charge_inference(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
		amount: i64,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("used_tokens"),
				Expr::col("used_tokens").add(Expr::value(amount)),
			)
			.and_where(Expr::col("request_id").eq(Expr::value(request)))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col("token_limit"))
					.sub(Expr::col("used_tokens"))
					.gte(Expr::value(amount)),
			)
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			!= 1
		{
			return Err(Error::Invalid(
				"generated agent token budget exhausted".into(),
			));
		}
		Ok(())
	}

	pub(crate) async fn charge_embedding(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
		amount: i64,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("embedding_calls"),
				Expr::col("embedding_calls").add(Expr::value(1_i64)),
			)
			.value_expr(
				Alias::new("used_tokens"),
				Expr::col("used_tokens").add(Expr::value(amount)),
			)
			.and_where(Expr::col("request_id").eq(Expr::value(request)))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col("embedding_calls"))
					.lt(Expr::col("embedding_call_limit")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col("token_limit"))
					.sub(Expr::col("used_tokens"))
					.gte(Expr::value(amount)),
			)
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			!= 1
		{
			return Err(Error::Invalid(
				"generated embedding call or token budget exhausted".into(),
			));
		}
		Ok(())
	}

	pub(crate) async fn refund(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
		amount: i64,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("used_tokens"),
				Expr::col("used_tokens").sub(Expr::value(amount)),
			)
			.and_where(Expr::col("request_id").eq(Expr::value(request)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

impl GenerationRequest {
	pub(crate) async fn released_policy(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
	) -> Result<Option<(String, String)>> {
		let (sql, values) = Query::select()
			.columns(["tenant", "policy_id"].map(Alias::new))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(request)))
			.and_where(Expr::col("quota_released").eq(reinhardt::query::Expr::value(true)))
			.build(PostgresQueryBuilder);
		tx.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| {
				Ok((
					row.get("tenant").map_err(FrameworkError::from)?,
					row.get("policy_id").map_err(FrameworkError::from)?,
				))
			})
			.transpose()
	}
}

impl GenerationPolicy {
	pub(crate) async fn refund_allocated(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		policy: &str,
		amount: i64,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("allocated_tokens"),
				Expr::col("allocated_tokens").sub(Expr::value(amount)),
			)
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("id").eq(Expr::value(policy)))
			.and_where(Expr::col("allocated_tokens").gte(Expr::value(amount)))
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			!= 1
		{
			return Err(Error::Conflict(
				"generation token refund exceeds allocated quota".into(),
			));
		}
		Ok(())
	}
}

impl GenerationUsage {
	pub(crate) async fn reserve(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
		attempt: Uuid,
		run: Uuid,
		amount: i64,
	) -> Result<()> {
		let record = Self::build()
			.request_id(request)
			.attempt_id(attempt)
			.run_id(run)
			.reserved_tokens(amount)
			.reported_tokens(None)
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	pub(crate) async fn report(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
		attempt: Uuid,
		reported: Option<i64>,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("reported_tokens"), Expr::value(reported))
			.and_where(Expr::col("request_id").eq(Expr::value(request)))
			.and_where(Expr::col("attempt_id").eq(Expr::value(attempt)))
			.and_where(Expr::col("reported_tokens").is_null())
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

pub(crate) struct EmbeddingAttempt<'a> {
	pub id: uuid::Uuid,
	pub workspace: uuid::Uuid,
	pub run: Option<uuid::Uuid>,
	pub entry: Option<uuid::Uuid>,
	pub purpose: &'a str,
	pub provider: &'a EntityRef,
	pub request_bytes: i64,
	pub amount: i64,
}

impl GenerationEmbeddingUsage {
	pub(crate) async fn reserve(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
		attempt: &EmbeddingAttempt<'_>,
	) -> Result<()> {
		Definition::read_in(tx, &attempt.provider.id, &attempt.provider.version).await?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(
				[
					"request_id",
					"attempt_id",
					"workspace_id",
					"run_id",
					"entry_id",
					"purpose",
					"provider_id",
					"provider_version",
					"request_bytes",
					"reserved_tokens",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(request),
				IntoValue::into_value(attempt.id),
				IntoValue::into_value(attempt.workspace),
				IntoValue::into_value(attempt.run),
				IntoValue::into_value(attempt.entry),
				IntoValue::into_value(attempt.purpose),
				IntoValue::into_value(&attempt.provider.id),
				IntoValue::into_value(&attempt.provider.version),
				IntoValue::into_value(attempt.request_bytes),
				IntoValue::into_value(attempt.amount),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn report(
		tx: &mut dyn TransactionExecutor,
		request: Uuid,
		attempt: Uuid,
		reported: Option<i64>,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("reported_tokens"), Expr::value(reported))
			.and_where(Expr::col("request_id").eq(Expr::value(request)))
			.and_where(Expr::col("attempt_id").eq(Expr::value(attempt)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

use reinhardt::query::IntoValue;
