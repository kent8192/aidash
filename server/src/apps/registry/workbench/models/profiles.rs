//! Tenant-scoped real-tool connection profile persistence.
use super::AgentTestProfile;
use crate::apps::registry::workbench::serializers::profile::{ProfileInput, TestProfile};
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::QueryRow;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

impl AgentTestProfile {
	pub(crate) async fn locked(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		id: &str,
	) -> Result<TestProfile> {
		use reinhardt::query::{ColumnRef, LockType};
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.ok_or_else(|| Error::NotFound("test profile".into()))?;
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(row).data,
		)?)
	}

	pub(crate) async fn page(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
	) -> Result<Vec<TestProfile>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.order_by(&["id"])
			.limit(100)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(Into::into)
			.collect())
	}

	pub(crate) async fn save(
		tx: &mut AtomicTransaction,
		tenant: &str,
		id: &str,
		input: &ProfileInput,
	) -> Result<TestProfile> {
		let existing = Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_id().eq(id))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.next();
		if existing.as_ref().map_or(0, |row| row.revision) != input.expected_revision {
			return Err(Error::Conflict("test profile changed".into()));
		}
		let rules = serde_json::to_value(&input.rules)?;
		if let Some(existing) = existing {
			let (sql, values) = Query::update()
				.table(Alias::new(Self::table_name()))
				.value_expr(
					Alias::new("revision"),
					Expr::col("revision").add(Expr::value(1_i64)),
				)
				.value_expr(Alias::new("enabled"), Expr::value(input.enabled))
				.value_expr(Alias::new("rules"), Expr::value(rules))
				.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
				.and_where(Expr::col("tenant").eq(Expr::value(&existing.tenant)))
				.and_where(Expr::col("id").eq(Expr::value(&existing.id)))
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
		} else {
			let (sql, values) = Query::insert()
				.into_table(Alias::new(Self::table_name()))
				.columns(["tenant", "id", "enabled", "rules"].map(Alias::new))
				.values_panic([
					IntoValue::into_value(tenant),
					IntoValue::into_value(id),
					IntoValue::into_value(input.enabled),
					IntoValue::into_value(rules),
				])
				.build(PostgresQueryBuilder);
			TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
		}
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_id().eq(id))
			.get_with_db(tx)
			.await?
			.into())
	}
}

use reinhardt::query::IntoValue;
