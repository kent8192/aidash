//! Query-generated bindings for Workspaces that predate Scratch Spaces.
use reinhardt::query::{
	Alias, Expr, Func, OnConflict, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};

fn a(name: &str) -> Alias {
	Alias::new(name)
}

fn owner(table: &str, column: &str, time_order: bool) -> SimpleExpr {
	let mut query = Query::select();
	query
		.column(a(column))
		.from(a(table))
		.and_where(Expr::col((a(table), a("workspace_id"))).equals((a("workspaces"), a("id"))))
		.and_where(Expr::col((a(table), a("tenant"))).equals((a("workspaces"), a("tenant"))));
	if time_order {
		query
			.order_by(a("created_at"), Order::Asc)
			.order_by(a("id"), Order::Asc);
	}
	query.limit(1);
	SimpleExpr::SubQuery(None, Box::new(query.to_owned()))
}

pub(crate) fn forward() -> String {
	let bindings = Query::update()
		.table(a("workspaces"))
		.value_expr(
			a("scratch_space_id"),
			Func::coalesce(vec![
				owner("conversations", "scratch_space_id", true),
				Expr::cust("gen_random_uuid()").into(),
			]),
		)
		.value_expr(
			a("created_by"),
			Func::coalesce(vec![
				owner("authorization_workspaces", "owner_subject", false),
				owner("conversations", "created_by", true),
				owner("tasks", "created_by", true),
			]),
		)
		.to_string(PostgresQueryBuilder);
	let mut spaces = Query::select();
	spaces
		.columns(["scratch_space_id", "tenant", "node", "title", "created_by"].map(a))
		.expr(Expr::value("active"))
		.expr(Expr::cust("CURRENT_TIMESTAMP"))
		.expr(Expr::cust("CURRENT_TIMESTAMP"))
		.from(a("workspaces"));
	let insert_spaces = Query::insert()
		.into_table(a("scratch_spaces"))
		.columns(
			[
				"id",
				"tenant",
				"node",
				"name",
				"creator",
				"state",
				"created_at",
				"last_used_at",
			]
			.map(a),
		)
		.from_subquery(spaces.to_owned())
		.on_conflict(OnConflict::column(a("id")).do_nothing().to_owned())
		.to_string(PostgresQueryBuilder);
	let mut settings = Query::select();
	settings
		.columns(["scratch_space_id", "tenant"].map(a))
		.expr(Expr::value(
			serde_json::to_value(aidash_domain::scratch::ScratchSettings::default())
				.expect("default settings serialize"),
		))
		.from(a("workspaces"));
	let insert_settings = Query::insert()
		.into_table(a("scratch_space_settings"))
		.columns(["space_id", "tenant", "settings"].map(a))
		.from_subquery(settings.to_owned())
		.on_conflict(OnConflict::column(a("space_id")).do_nothing().to_owned())
		.to_string(PostgresQueryBuilder);
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{bindings};\n{insert_spaces};\n{insert_settings};\n"
	)
}

pub(crate) fn backward() -> String {
	format!(
		"{};\n",
		Query::select()
			.expr(Expr::value(1))
			.to_string(PostgresQueryBuilder)
	)
}
