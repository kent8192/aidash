//! Query-generated backfill for conversations created before Scratch Spaces.
use reinhardt::query::{
	Alias, Expr, JoinType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
pub(crate) fn forward() -> String {
	let ids = Query::update()
		.table(Alias::new("conversations"))
		.value_expr(
			Alias::new("scratch_space_id"),
			Expr::cust("gen_random_uuid()"),
		)
		.to_string(PostgresQueryBuilder);
	let mut spaces = Query::select();
	spaces
		.columns(
			[
				("c", "scratch_space_id"),
				("c", "tenant"),
				("w", "node"),
				("w", "title"),
				("c", "created_by"),
			]
			.map(|(a, b)| (Alias::new(a), Alias::new(b))),
		)
		.expr(Expr::value("active"))
		.expr(Expr::cust("CURRENT_TIMESTAMP"))
		.expr(Expr::cust("CURRENT_TIMESTAMP"))
		.from_as(Alias::new("conversations"), Alias::new("c"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(Alias::new("workspaces"), Alias::new("w")),
			Expr::col((Alias::new("c"), Alias::new("workspace_id")))
				.equals((Alias::new("w"), Alias::new("id"))),
		);
	let insert_spaces = Query::insert()
		.into_table(Alias::new("scratch_spaces"))
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
			.map(Alias::new),
		)
		.from_subquery(spaces.to_owned())
		.to_string(PostgresQueryBuilder);
	let mut settings = Query::select();
	settings
		.columns(["scratch_space_id", "tenant"].map(Alias::new))
		.expr(Expr::value(
			serde_json::to_value(aidash_domain::scratch::ScratchSettings::default())
				.expect("default settings serialize"),
		))
		.from(Alias::new("conversations"));
	let insert_settings = Query::insert()
		.into_table(Alias::new("scratch_space_settings"))
		.columns(["space_id", "tenant", "settings"].map(Alias::new))
		.from_subquery(settings.to_owned())
		.to_string(PostgresQueryBuilder);
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{ids};\n{insert_spaces};\n{insert_settings};\n"
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
