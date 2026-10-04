//! Durable sandbox admission, progress, stop, and expiry.
use super::AgentTestSession;
use crate::apps::registry::workbench::serializers::contracts::Draft;
use crate::apps::registry::workbench::serializers::test::{TestLimits, TestSession};
use crate::apps::registry::workbench::serializers::trust::EvidenceRow;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;

async fn database_time(tx: &mut dyn TransactionExecutor) -> Result<DateTime<Utc>> {
	let (sql, values) = Query::select()
		.expr_as(
			SimpleExpr::FunctionCall("clock_timestamp".into_iden(), vec![]),
			Alias::new("now"),
		)
		.build(PostgresQueryBuilder);
	Ok(tx
		.fetch_one(&sql, convert_values(values))
		.await?
		.get("now")
		.map_err(FrameworkError::from)?)
}

impl AgentTestSession {
	pub(crate) async fn evidence_page(
		tx: &mut dyn TransactionExecutor,
		draft: Uuid,
		revision: i64,
		limit: u64,
	) -> Result<Vec<EvidenceRow>> {
		use reinhardt::query::Order;
		let (sql, values) = Query::select()
			.columns(
				[
					"id",
					"status",
					"scenario",
					"usage",
					"created_at",
					"expires_at",
					"expired_at",
				]
				.map(Alias::new),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("draft_id").eq(Expr::value(draft)))
			.and_where(Expr::col("revision").eq(Expr::value(revision)))
			.order_by(Alias::new("created_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(limit)
			.build(PostgresQueryBuilder);
		tx.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				serde_json::from_value(QueryRow::from_backend_row(row).data).map_err(Into::into)
			})
			.collect()
	}

	pub(crate) async fn read(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		lock: bool,
	) -> Result<TestSession> {
		let records = Self::objects().filter(Self::field_id().eq(id));
		let records = if lock {
			records.select_for_update().all_with_executor(tx).await
		} else {
			records.all_with_executor(tx).await
		}
		.map_err(FrameworkError::from)?;
		records
			.into_iter()
			.next()
			.map(Into::into)
			.ok_or_else(|| Error::NotFound("test session".into()))
	}

	pub(crate) async fn page(
		tx: &mut dyn TransactionExecutor,
		draft: Uuid,
	) -> Result<Vec<TestSession>> {
		Ok(Self::objects()
			.filter(Self::field_draft_id().eq(draft))
			.order_by(&["-created_at", "-id"])
			.limit(100)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(Into::into)
			.collect())
	}

	async fn expire_unfinished(
		tx: &mut dyn TransactionExecutor,
		tenant: Option<&str>,
		deadline: DateTime<Utc>,
	) -> Result<()> {
		let mut query = Query::update();
		query
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("status"), Expr::value("outcome_unknown"))
			.value_expr(Alias::new("active_slot"), Expr::value(None::<Uuid>))
			.value_expr(
				Alias::new("error"),
				Expr::value("test worker stopped before recording a final outcome"),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("status").eq(Expr::value("running")))
			.and_where(Expr::col("created_at").lt(Expr::value(deadline)));
		if let Some(tenant) = tenant {
			query.and_where(Expr::col("tenant").eq(Expr::value(tenant)));
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn purge(tx: &mut dyn TransactionExecutor) -> Result<u64> {
		let now = database_time(tx).await?;
		Self::expire_unfinished(tx, None, now - Duration::seconds(930)).await?;
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("scenario"),
				Expr::col("scenario").sub(Expr::value("fixtures")),
			)
			.value_expr(Alias::new("conversation"), Expr::value(None::<Value>))
			.value_expr(Alias::new("tool_calls"), Expr::value(None::<Value>))
			.value_expr(Alias::new("expired_at"), Expr::value(now))
			.and_where(Expr::col("expires_at").lte(Expr::value(now)))
			.and_where(Expr::col("expired_at").is_null())
			.build(PostgresQueryBuilder);
		Ok(tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected)
	}

	/// The caller holds the tenant limits row lock across admission and commit.
	pub(crate) async fn admit(
		tx: &mut dyn TransactionExecutor,
		draft: &Draft,
		limits: &TestLimits,
		scenario: Value,
		conversation: Value,
	) -> Result<TestSession> {
		let now = database_time(tx).await?;
		Self::expire_unfinished(
			tx,
			Some(&draft.tenant),
			now - Duration::seconds(i64::from(limits.max_duration_secs) + 30),
		)
		.await?;
		let active = Self::objects()
			.filter(Self::field_tenant().eq(&draft.tenant))
			.filter(Self::field_status().eq("running"))
			.values(&["id"])
			.count_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		if active >= limits.max_concurrent as usize {
			return Err(Error::Conflict(
				"tenant test concurrency limit reached".into(),
			));
		}
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(
				[
					"id",
					"draft_id",
					"tenant",
					"revision",
					"status",
					"scenario",
					"conversation",
					"tool_calls",
					"usage",
					"active_slot",
					"expires_at",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(Uuid::now_v7()),
				IntoValue::into_value(draft.id),
				IntoValue::into_value(&draft.tenant),
				IntoValue::into_value(draft.revision),
				IntoValue::into_value("running"),
				IntoValue::into_value(scenario),
				IntoValue::into_value(conversation),
				IntoValue::into_value(json!([])),
				IntoValue::into_value(json!({})),
				IntoValue::into_value(draft.id),
				IntoValue::into_value(now + Duration::days(i64::from(limits.payload_days))),
			])
			.returning_all()
			.build(PostgresQueryBuilder);
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(tx.fetch_one(&sql, convert_values(values)).await?).data,
		)?)
	}

	pub(crate) async fn finish(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		status: &str,
		conversation: Value,
		calls: Value,
		usage: Value,
		error: Option<String>,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("status"), Expr::value(status))
			.value_expr(Alias::new("conversation"), Expr::value(conversation))
			.value_expr(Alias::new("tool_calls"), Expr::value(calls))
			.value_expr(Alias::new("usage"), Expr::value(usage))
			.value_expr(Alias::new("error"), Expr::value(error))
			.value_expr(Alias::new("active_slot"), Expr::value(None::<Uuid>))
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("status").eq(Expr::value("running")))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn stop(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<TestSession> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("status"), Expr::value("stopped"))
			.value_expr(Alias::new("active_slot"), Expr::value(None::<Uuid>))
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("status").eq(Expr::value("running")))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Self::read(tx, id, false).await
	}

	pub(crate) async fn calls(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		calls: Value,
		running: bool,
	) -> Result<()> {
		let mut query = Query::update();
		query
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("tool_calls"), Expr::value(calls))
			.and_where(Expr::col("id").eq(Expr::value(id)));
		if running {
			query.and_where(Expr::col("status").eq(Expr::value("running")));
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn progress(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		conversation: Value,
		calls: Value,
		usage: Value,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("conversation"), Expr::value(conversation))
			.value_expr(Alias::new("tool_calls"), Expr::value(calls))
			.value_expr(Alias::new("usage"), Expr::value(usage))
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("status").eq(Expr::value("running")))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

use reinhardt::query::IntoValue;
