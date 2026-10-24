//! Typed dependency selectors for the deletion transaction's immutable snapshot.
use reinhardt::query::{
	Alias, BinOper, Expr, ExprTrait, Func, Query, SimpleExpr, types::PgBinOper,
};

pub(super) fn column(table: &str, field: &str) -> SimpleExpr {
	Expr::col((Alias::new(table), Alias::new(field))).into()
}
pub(super) fn json_text(value: SimpleExpr, key: &str) -> SimpleExpr {
	value.binary(
		BinOper::PgOperator(PgBinOper::JsonGetAsText),
		Expr::value(key).into(),
	)
}
fn boolean(key: &str, expected: bool) -> SimpleExpr {
	Func::coalesce(vec![
		Expr::col("result")
			.binary(
				BinOper::PgOperator(PgBinOper::JsonGetByIndex),
				Expr::value(key),
			)
			.eq(Expr::value(serde_json::json!(expected))),
		Expr::value(false).into(),
	])
}
pub(super) fn unsafe_operation() -> SimpleExpr {
	let stopped = boolean("termination_confirmed", true).and(boolean("runner_acknowledged", true));
	let never_dispatched = Expr::col("runner_instance")
		.is_null()
		.and(boolean("effects_may_have_occurred", false));
	Expr::col("state")
		.is_in(["completed", "cancelled", "failed", "withdrawn"])
		.and(stopped.clone().or(never_dispatched))
		.or(Expr::col("state").eq("uncertain").and(stopped))
		.not()
}
fn workspace(table: &str, node: &str) -> SimpleExpr {
	Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from_as(Alias::new("workspaces"), Alias::new("w"))
			.and_where(column("w", "id").eq(column(table, "workspace_id")))
			.and_where(column("w", "tenant").eq(column(table, "tenant")))
			.and_where(column("w", "node").eq(Expr::value(node)))
			.to_owned(),
	)
	.into()
}
fn area(table: &str, node: &str, address: &str) -> SimpleExpr {
	Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from_as(Alias::new("core_areas"), Alias::new("a"))
			.and_where(column("a", "id").eq(column(table, "area_id")))
			.and_where(column("a", "tenant").eq(column(table, "tenant")))
			.and_where(column("a", "home_node").eq(Expr::value(address)))
			.and_where(workspace("a", node))
			.to_owned(),
	)
	.into()
}
fn capability(table: &str, node: &str, address: &str) -> SimpleExpr {
	let binding = if table == "core_operations" {
		column("r", "id").eq(column(table, "run_id"))
	} else {
		Expr::from(column("r", "id"))
			.in_subquery(
				Query::select()
					.column((Alias::new("o"), Alias::new("run_id")))
					.from_as(Alias::new("core_operations"), Alias::new("o"))
					.and_where(
						column("o", "id")
							.cast_as("text")
							.eq(json_text(column(table, "data"), "operation_id")),
					)
					.and_where(column("o", "tenant").eq(column(table, "tenant")))
					.to_owned(),
			)
			.into()
	};
	area(table, node, address).or(Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from_as(Alias::new("runs"), Alias::new("r"))
			.and_where(column("r", "tenant").eq(column(table, "tenant")))
			.and_where(column("r", "node").eq(Expr::value(node)))
			.and_where(binding)
			.to_owned(),
	))
}
pub(super) fn selectors(
	node: &str,
	address: &str,
	deployment: &str,
	deletion: uuid::Uuid,
) -> Vec<(&'static str, &'static str, SimpleExpr, SimpleExpr)> {
	let run = Expr::col("node")
		.eq(Expr::value(node))
		.or(workspace("runs", node).and(Expr::col("home_node").eq(Expr::value(address))))
		.or(Expr::exists(
			Query::select()
				.expr(Expr::value(1_i32))
				.from_as(Alias::new("core_runs"), Alias::new("cr"))
				.and_where(column("cr", "run_id").eq(column("runs", "id")))
				.and_where(column("cr", "tenant").eq(column("runs", "tenant")))
				.and_where(area("cr", node, address))
				.to_owned(),
		));
	let intent = Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from_as(Alias::new("tasks"), Alias::new("t"))
			.and_where(column("t", "id").eq(column("generation_remote_intents", "task_id")))
			.and_where(column("t", "tenant").eq(column("generation_remote_intents", "tenant")))
			.and_where(workspace("t", node))
			.to_owned(),
	);
	let delegated_task = Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from_as(
				Alias::new("authorization_remote_execution"),
				Alias::new("e"),
			)
			.and_where(column("e", "task_id").eq(column("tasks", "id")))
			.and_where(column("e", "tenant").eq(column("tasks", "tenant")))
			.and_where(column("e", "task_revision").eq(column("tasks", "revision")))
			.and_where(Expr::exists(
				Query::select()
					.expr(Expr::value(1_i32))
					.from_as(Alias::new("authorization_remote_grants"), Alias::new("g"))
					.and_where(column("g", "id").eq(column("e", "grant_id")))
					.and_where(column("g", "tenant").eq(column("e", "tenant")))
					.and_where(column("g", "node_id").eq(Expr::value(address)))
					.to_owned(),
			))
			.to_owned(),
	);
	// A locally owned Task may execute on another Node. Its captured Run must
	// also fence reassignment while decided atomic recovery defers cancellation.
	// Exact local Home/Workspace binding excludes a foreign Home's UUIDs.
	let local_task = Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from_as(Alias::new("runs"), Alias::new("r"))
			.and_where(column("r", "task_id").eq(column("tasks", "id")))
			.and_where(column("r", "workspace_id").eq(column("tasks", "workspace_id")))
			.and_where(column("r", "tenant").eq(column("tasks", "tenant")))
			.and_where(column("r", "phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.and_where(
				Expr::from(column("r", "id")).in_subquery(
					Query::select()
						.column(Alias::new("work_id"))
						.from(Alias::new("node_cancellation_jobs"))
						.and_where(Expr::col("deletion_id").eq(Expr::value(deletion)))
						.and_where(Expr::col("kind").eq("run"))
						.to_owned(),
				),
			)
			.and_where(Expr::exists(
				Query::select()
					.expr(Expr::value(1_i32))
					.from_as(Alias::new("workspaces"), Alias::new("w"))
					.and_where(column("w", "id").eq(column("tasks", "workspace_id")))
					.and_where(column("w", "tenant").eq(column("tasks", "tenant")))
					.and_where(column("r", "home_node").eq(
						Expr::value(format!("{deployment}/")).binary(
							BinOper::PgOperator(PgBinOper::Concatenate),
							column("w", "node"),
						),
					))
					.to_owned(),
			))
			.to_owned(),
	);
	vec![
		(
			"run",
			"runs",
			run,
			Expr::col("phase")
				.is_not_in(["COMPLETED", "FAILED", "CANCELLED"])
				.or(Expr::col("lease_owner").is_not_null()),
		),
		(
			"admission",
			"authorization_remote_admissions",
			Expr::col("node").eq(Expr::value(node)),
			Expr::col("cancelled").eq(false),
		),
		(
			"generation",
			"generation_requests",
			Expr::col("node").eq(Expr::value(node)),
			Expr::col("status")
				.is_in(["PENDING_APPROVAL", "QUEUED", "ACTIVE"])
				.or(Expr::col("quota_released").eq(false)),
		),
		(
			"grant",
			"authorization_remote_grants",
			workspace("authorization_remote_grants", node)
				.or(Expr::col("node_id").eq(Expr::value(address))),
			Expr::value(true).into(),
		),
		(
			"intent",
			"generation_remote_intents",
			intent.into(),
			Expr::col("cancel_delivered").eq(false),
		),
		(
			"task",
			"tasks",
			workspace("tasks", node).or(delegated_task).or(local_task),
			Expr::col("status").is_not_in(["COMPLETED", "CANCELLED", "ABANDONED"]),
		),
		(
			"operation",
			"core_operations",
			capability("core_operations", node, address),
			unsafe_operation(),
		),
		(
			"python",
			"core_records",
			capability("core_records", node, address),
			Expr::col("kind")
				.eq("python_session")
				.and(Expr::col("state").eq("frozen")),
		),
	]
}
