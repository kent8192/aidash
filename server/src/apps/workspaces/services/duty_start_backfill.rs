//! Query-backed migration for the Duty provenance recorded on Tasks and their events.
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};

const DEPLOYMENT_SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";
const TASK_EVENTS: [&str; 5] = [
	"task.created",
	"task.claimed",
	"task.updated",
	"task.completed",
	"task.abandoned",
];

pub(crate) fn forward() -> String {
	let task = |field| (Alias::new("tasks"), Alias::new(field));
	let tick = |field| (Alias::new("duty_ticks"), Alias::new(field));
	let automatic_tick = Query::select()
		.column(tick("duty_id"))
		.from(Alias::new("duty_ticks"))
		.and_where(Expr::col(tick("tenant")).equals(task("tenant")))
		.and_where(Expr::col(tick("task_id")).equals(task("id")))
		.and_where(Expr::col(tick("started_by")).is_null())
		.order_by(tick("recorded_at"), Order::Desc)
		.order_by(tick("id"), Order::Desc)
		.limit(1)
		.to_owned();
	let tasks = Query::update()
		.table(Alias::new("tasks"))
		.value_expr(
			Alias::new("started_by_duty"),
			SimpleExpr::CustomWithExpr(
				"COALESCE(?, CASE WHEN \"duty_script\" THEN \"duty_id\" END)".into(),
				vec![Expr::subquery(automatic_tick.clone()).into()],
			),
		)
		.cond_where(
			Condition::any()
				.add(Expr::col(task("duty_script")).eq(true))
				.add(Expr::exists(automatic_tick)),
		)
		.to_string(PostgresQueryBuilder);

	// Completion and abandonment wrap the Task. Older events can also carry
	// other data, so edit just the Task field and keep the rest of the payload.
	let event_task = r#"COALESCE("events"."data" #>> '{task,id}', "events"."data" ->> 'id')"#;
	let task_uuid = Expr::cust(format!(
		"CASE WHEN pg_input_is_valid({event_task}, 'uuid') THEN ({event_task})::uuid END"
	));
	let matching_task = Query::select()
		.column(task("started_by_duty"))
		.from(Alias::new("tasks"))
		.and_where(Expr::col(task("tenant")).equals((Alias::new("events"), Alias::new("tenant"))))
		.and_where(Expr::col(task("started_by_duty")).is_not_null())
		.and_where(Expr::col(task("id")).eq(task_uuid))
		.to_owned();
	let events = Query::update()
		.table(Alias::new("events"))
		.value_expr(
			Alias::new("data"),
			SimpleExpr::CustomWithExpr(
				"jsonb_set(\"data\", CASE WHEN jsonb_typeof(\"data\" -> 'task') = 'object' THEN ARRAY['task', 'started_by_duty'] ELSE ARRAY['started_by_duty'] END, to_jsonb(?))".into(),
				vec![Expr::subquery(matching_task.clone()).into()],
			),
		)
		.and_where(Expr::col(Alias::new("kind")).is_in(TASK_EVENTS))
		.and_where(Expr::exists(matching_task))
		.to_string(PostgresQueryBuilder);
	format!("{DEPLOYMENT_SCOPE}{tasks};\n{events};\n")
}

pub(crate) fn backward() -> String {
	let events = Query::update()
		.table(Alias::new("events"))
		.value_expr(
			Alias::new("data"),
			Expr::cust("\"data\" - 'started_by_duty' #- '{task,started_by_duty}'"),
		)
		.and_where(Expr::col(Alias::new("kind")).is_in(TASK_EVENTS))
		.and_where(Expr::cust(
			"\"data\" ? 'started_by_duty' OR \"data\" #> '{task,started_by_duty}' IS NOT NULL",
		))
		.to_string(PostgresQueryBuilder);
	format!("{DEPLOYMENT_SCOPE}{events};\n")
}
