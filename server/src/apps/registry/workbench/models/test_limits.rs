//! Tenant-scoped sandbox limits serialized with admissions and changes.
use super::AgentTestLimit;
use crate::Result;
use crate::apps::registry::workbench::serializers::test::TestLimits;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

impl AgentTestLimit {
	pub(crate) async fn save(tx: &mut dyn TransactionExecutor, limits: &TestLimits) -> Result<()> {
		let columns = [
			"max_input_bytes",
			"max_output_tokens",
			"max_total_tokens",
			"max_steps",
			"max_duration_secs",
			"max_concurrent",
			"payload_days",
			"incident_evidence_days",
		];
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(std::iter::once("tenant").chain(columns).map(Alias::new))
			.values_panic([
				IntoValue::into_value(&limits.tenant),
				IntoValue::into_value(limits.max_input_bytes),
				IntoValue::into_value(limits.max_output_tokens),
				IntoValue::into_value(limits.max_total_tokens),
				IntoValue::into_value(limits.max_steps),
				IntoValue::into_value(limits.max_duration_secs),
				IntoValue::into_value(limits.max_concurrent),
				IntoValue::into_value(limits.payload_days),
				IntoValue::into_value(limits.incident_evidence_days),
			])
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.update_columns(columns.map(Alias::new))
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
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
