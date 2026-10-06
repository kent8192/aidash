//! Human requests and their answers on caller-owned native transactions.
use super::HumanRequest;
use crate::apps::execution::serializers::human_requests::HumanRequest as Contract;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, QueryRow, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

impl HumanRequest {
	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		lock: bool,
	) -> Result<Option<Contract>> {
		let query = Self::objects().filter(Self::field_id().eq(id));
		let records = if lock {
			query.select_for_update().all_with_executor(tx).await
		} else {
			query.all_with_executor(tx).await
		};
		Ok(records.map_err(FrameworkError::from)?.pop().map(Into::into))
	}

	pub(crate) async fn admit(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
		run: Uuid,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<(Contract, bool)> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(
				[
					"id",
					"workspace_id",
					"run_id",
					"kind",
					"prompt",
					"request_key",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(Uuid::new_v4()),
				IntoValue::into_value(workspace),
				IntoValue::into_value(run),
				IntoValue::into_value(kind),
				IntoValue::into_value(prompt),
				IntoValue::into_value(key),
			])
			.on_conflict(OnConflict::columns(["request_key"]).do_nothing())
			.returning_all()
			.build(PostgresQueryBuilder);
		if let Some(row) = tx.fetch_optional(&sql, convert_values(values)).await? {
			return Ok((
				serde_json::from_value(QueryRow::from_backend_row(row).data)?,
				true,
			));
		}
		let request = Self::objects()
			.filter(Self::field_request_key().eq(key))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::NotFound("human request".into()))?;
		Ok((request.into(), false))
	}

	pub(crate) async fn answer_locked(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		response: Value,
		actor: &str,
	) -> Result<Contract> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("response"), Expr::value(response))
			.value_expr(Alias::new("answered_by"), Expr::value(actor))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.returning_all()
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(row).data,
		)?)
	}
}

use reinhardt::query::IntoValue;
