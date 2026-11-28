//! Recreate the historical Live preview pin when reversing execution 0182.
use reinhardt::query::{
	Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

pub(crate) fn backward() -> String {
	let area = Query::select()
		.column("area_id")
		.from("core_runs")
		.and_where(Expr::col(("core_runs", "run_id")).equals(("core_live_previews", "run_id")))
		.and_where(Expr::col(("core_runs", "tenant")).equals(("core_live_previews", "tenant")))
		.to_owned();
	let bind = Query::update()
		.table("core_live_previews")
		.value_expr("area_id", Expr::subquery(area))
		.to_string(PostgresQueryBuilder);
	let revision = Query::select()
		.column("revision")
		.from("core_areas")
		.and_where(Expr::col(("core_areas", "id")).equals(("core_live_previews", "area_id")))
		.and_where(Expr::col(("core_areas", "tenant")).equals(("core_live_previews", "tenant")))
		.to_owned();
	let pin = Query::update()
		.table("core_live_previews")
		.value_expr("revision", Expr::subquery(revision))
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{bind};\n{pin};\n")
}
