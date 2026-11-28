//! Direction-specific translations between a stored Area link and its owner.
use reinhardt::query::{
	Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _, SimpleExpr,
};

pub(crate) fn forward() -> String {
	let subject = Query::select()
		.column("owner")
		.from("core_areas")
		.and_where(Expr::col(("core_areas", "id")).equals(("core_runs", "area_id")))
		.and_where(Expr::col(("core_areas", "tenant")).equals(("core_runs", "tenant")))
		.to_owned();
	let sql = Query::update()
		.table("core_runs")
		.value_expr("scope_subject", Expr::subquery(subject))
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{sql};\n")
}

pub(crate) fn backward() -> String {
	let area = Query::select()
		.column(("area", "id"))
		.from_as(Alias::new("core_areas"), Alias::new("area"))
		.join(
			reinhardt::query::JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("task_executions"),
				Alias::new("run"),
			),
			Expr::col(("area", "tenant")).equals(("run", "tenant")),
		)
		.and_where(Expr::col(("area", "home_node")).equals(("run", "home_node")))
		.and_where(Expr::col(("area", "thread_id")).equals(("run", "thread_id")))
		.and_where(Expr::col(("area", "agent_id")).equals(("run", "agent_id")))
		.and_where(Expr::col(("run", "id")).equals(("core_runs", "run_id")))
		.and_where(Expr::col(("run", "tenant")).equals(("core_runs", "tenant")))
		.and_where(Expr::col(("area", "owner")).equals(("core_runs", "scope_subject")))
		.to_owned();
	let sql = Query::update()
		.table("core_runs")
		.value_expr(
			"area_id",
			SimpleExpr::CustomWithExpr(
				"COALESCE(?, '00000000-0000-0000-0000-000000000000'::uuid)".into(),
				vec![Expr::subquery(area).into()],
			),
		)
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{sql};\n")
}
