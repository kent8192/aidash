//! Tenant-scoped sandbox limits serialized with admissions and changes.
use super::AgentTestLimit;
use crate::apps::registry::workbench::serializers::test::TestLimits;
use crate::{Error, Result};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

impl AgentTestLimit {
	/// Writes a Tenant's limits at the revision they were read at; a write in
	/// between is a conflict.
	pub(crate) async fn save(
		tx: &mut dyn TransactionExecutor,
		limits: &TestLimits,
	) -> Result<TestLimits> {
		if Self::locked(tx, &limits.tenant).await?.revision != limits.revision {
			return Err(Error::Conflict("test limits changed".into()));
		}
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("max_input_bytes"),
				Expr::value(limits.max_input_bytes),
			)
			.value_expr(
				Alias::new("max_output_tokens"),
				Expr::value(limits.max_output_tokens),
			)
			.value_expr(
				Alias::new("max_total_tokens"),
				Expr::value(limits.max_total_tokens),
			)
			.value_expr(Alias::new("max_steps"), Expr::value(limits.max_steps))
			.value_expr(
				Alias::new("max_duration_secs"),
				Expr::value(limits.max_duration_secs),
			)
			.value_expr(
				Alias::new("max_concurrent"),
				Expr::value(limits.max_concurrent),
			)
			.value_expr(Alias::new("payload_days"), Expr::value(limits.payload_days))
			.value_expr(
				Alias::new("incident_evidence_days"),
				Expr::value(limits.incident_evidence_days),
			)
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.and_where(Expr::col("tenant").eq(Expr::value(&limits.tenant)))
			.returning_all()
			.build(PostgresQueryBuilder);
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(tx.fetch_one(&sql, convert_values(values)).await?).data,
		)?)
	}

	pub(crate) async fn locked(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
	) -> Result<TestLimits> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns([Alias::new("tenant")])
			.values_panic([IntoValue::into_value(tenant)])
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(tx.fetch_one(&sql, convert_values(values)).await?).data,
		)?)
	}
}

use reinhardt::query::IntoValue;
