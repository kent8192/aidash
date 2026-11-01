//! Query-generated Run creation-time backfill for pre-migration records.
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _};

pub(crate) fn forward() -> String {
	let update = Query::update()
		.table((Alias::new("public"), Alias::new("runs")))
		.value_expr(
			Alias::new("created_at"),
			Expr::col(Alias::new("updated_at")),
		)
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{update};\n")
}

pub(crate) fn backward() -> String {
	format!(
		"{};\n",
		Query::select()
			.expr(Expr::value(1))
			.to_string(PostgresQueryBuilder)
	)
}
