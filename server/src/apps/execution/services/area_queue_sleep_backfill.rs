//! Sets the queue sleep flag from existing execution state in execution 0153.
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

pub(crate) fn forward() -> String {
	let execution = Alias::new("task_executions");
	let core = Alias::new("core_runs");
	let asleep = Query::select()
		.expr(Expr::value(1))
		.from(execution.clone())
		.and_where(
			Expr::col((execution.clone(), Alias::new("id")))
				.equals((core.clone(), Alias::new("run_id"))),
		)
		.and_where(
			Expr::col((execution.clone(), Alias::new("tenant")))
				.equals((core.clone(), Alias::new("tenant"))),
		)
		.and_where(SimpleExpr::FunctionCall(
			Alias::new("aidash_area_queue_should_sleep").into_iden(),
			["phase", "control", "pending", "lease_owner"]
				.into_iter()
				.map(|column| Expr::col((execution.clone(), Alias::new(column))).into())
				.collect(),
		))
		.to_owned();
	let update = Query::update()
		.table(core)
		.value_expr(Alias::new("asleep"), Expr::value(true))
		.and_where(Expr::exists(asleep))
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{update};\n")
}
