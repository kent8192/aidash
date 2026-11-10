//! Query-generated baseline data for the transaction visibility barrier.
use reinhardt::query::{Alias, PostgresQueryBuilder, Query, QueryStatementBuilder, Value};

pub(crate) fn forward() -> String {
	Query::insert()
		.into_table((Alias::new("public"), Alias::new("atomic_gate")))
		.columns(["singleton", "transaction_id", "commit_epoch"].map(Alias::new))
		.values_panic([true.into(), Value::Uuid(None), 0_i64.into()])
		.to_string(PostgresQueryBuilder)
}
