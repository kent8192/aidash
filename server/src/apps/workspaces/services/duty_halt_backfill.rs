//! The stored halts of Duties after Schedules became Duties (#1011): a Duty
//! whose Requester left is `requester_left`, and one stopped by the removed
//! minimum interval (#1012) is paused, keeping why in words.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

fn rename(from: &str, to: &str) -> String {
	Query::update()
		.table(Alias::new("duties"))
		.value(Alias::new("halt"), to)
		.and_where(Expr::col("halt").eq(from))
		.to_string(PostgresQueryBuilder)
}

pub(crate) fn forward() -> String {
	let paused = Query::update()
		.table(Alias::new("duties"))
		.value(Alias::new("halt"), "paused")
		.and_where(Expr::col("halt").eq("interval_too_short"))
		.to_string(PostgresQueryBuilder);
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n{paused};\n",
		rename("registrant_left", "requester_left")
	)
}

/// A paused Duty stays paused: nothing tells which were stopped by the
/// minimum interval.
pub(crate) fn backward() -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n",
		rename("requester_left", "registrant_left")
	)
}
