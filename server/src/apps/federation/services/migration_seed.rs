//! Query-generated baseline data for the transaction visibility barrier.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, Value,
};

pub(crate) fn forward() -> String {
	Query::insert()
		.into_table((Alias::new("public"), Alias::new("atomic_gate")))
		.columns(["singleton", "transaction_id", "commit_epoch"].map(Alias::new))
		.values_panic([true.into(), Value::Uuid(None), 0_i64.into()])
		.to_string(PostgresQueryBuilder)
}

pub(crate) fn backward() -> String {
	Query::delete()
		.from_table((Alias::new("public"), Alias::new("atomic_gate")))
		.and_where(Expr::col(Alias::new("singleton")).eq(true))
		.to_string(PostgresQueryBuilder)
}
