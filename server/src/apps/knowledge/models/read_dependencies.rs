//! Durable semantic dependency reads on the enclosing authority transaction.
use super::{SemanticAgentMemory, SemanticEntry, SemanticPoint, SemanticRunRead};
use crate::Result;
use crate::apps::knowledge::serializers::contracts::Entry;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

impl SemanticEntry {
	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
	) -> Result<Option<Entry>> {
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| serde_json::from_value(QueryRow::from_backend_row(row).data))
			.transpose()?)
	}
}

impl SemanticRunRead {
	pub(crate) async fn for_run(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<Vec<(Uuid, i64)>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.order_by(&["entry_id", "revision"])
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|row| (row.entry_id(), row.revision))
			.collect())
	}
}

impl SemanticAgentMemory {
	pub(crate) async fn identity_in(
		tx: &mut dyn TransactionExecutor,
		entry: Uuid,
	) -> Result<Option<(String, String)>> {
		Ok(Self::objects()
			.filter(Self::field_entry_id().eq(entry))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|row| (row.agent_id, row.agent_version)))
	}
}

impl SemanticPoint {
	pub(crate) async fn digest_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
	) -> Result<Option<String>> {
		Ok(Self::objects()
			.filter(Self::field_id().eq(id))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.and_then(|row| row.content_digest))
	}
}
