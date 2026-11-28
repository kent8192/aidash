//! Reconstruct a valid historical order when reversing execution 0181.
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};

pub(crate) fn backward() -> String {
	let earlier = Query::select()
		.expr(Expr::cust("count(*)"))
		.from_as(Alias::new("core_runs"), Alias::new("earlier"))
		.and_where(Expr::col(("earlier", "area_id")).equals(("core_runs", "area_id")))
		.and_where(Expr::col(("earlier", "run_id")).lte(Expr::col(("core_runs", "run_id"))))
		.to_owned();
	let generation = Query::select()
		.column("generation")
		.from("core_areas")
		.and_where(Expr::col(("core_areas", "id")).equals(("core_runs", "area_id")))
		.and_where(Expr::col(("core_areas", "tenant")).equals(("core_runs", "tenant")))
		.to_owned();
	let restore_bindings = Query::update()
		.table("core_runs")
		.value_expr("sequence", Expr::subquery(earlier))
		.value_expr(
			"generation",
			SimpleExpr::CustomWithExpr(
				"COALESCE(?, 1)".into(),
				vec![Expr::subquery(generation).into()],
			),
		)
		.to_string(PostgresQueryBuilder);
	let count = Query::select()
		.expr(Expr::cust("count(*) + 1"))
		.from("core_runs")
		.and_where(Expr::col(("core_runs", "area_id")).equals(("core_areas", "id")))
		.to_owned();
	let restore_cursor = Query::update()
		.table("core_areas")
		.value_expr("next_sequence", Expr::subquery(count))
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{restore_bindings};\n{restore_cursor};\n")
}
