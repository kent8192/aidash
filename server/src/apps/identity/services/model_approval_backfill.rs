//! Materialize a tenant's existing model consent on its other Nodes.
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, TableRef,
};

pub(crate) fn forward() -> String {
	let col = |table, field| (Alias::new(table), Alias::new(field));
	let approval = |field| col("authorization_catalog", field);
	let registry = |field| col("registry", field);
	let keys = ["tenant", "node", "entry_id", "entry_version"];
	let fields = [
		"tenant",
		"node",
		"entry_id",
		"entry_version",
		"enabled",
		"carry_over",
		"scope",
		"restriction",
		"carried_from_version",
		"carried_from_approver",
		"revision",
	];
	// Prefer an enabled consent when the old, per-Node decisions differ. The
	// source Node breaks ties deterministically. Keep each target's revision fence.
	let live = |table, node| {
		let mut deleted = Query::select();
		deleted
			.expr(Expr::value(1))
			.from(Alias::new("node_deletions"))
			.and_where(Expr::col(col("node_deletions", "tenant")).equals(col(table, "tenant")))
			.and_where(Expr::col(col("node_deletions", "node")).equals(col(table, node)))
			.and_where(Expr::col("restored_at").is_null());
		Expr::exists(deleted).not()
	};
	let source = Query::select()
		.distinct_on(["tenant", "entry_id", "entry_version"].map(|f| approval(f)))
		.columns(fields.map(approval))
		.from(Alias::new("authorization_catalog"))
		.inner_join(
			Alias::new("registry"),
			Expr::col(registry("tenant"))
				.equals(approval("tenant"))
				.and(Expr::col(registry("id")).equals(approval("entry_id")))
				.and(Expr::col(registry("version")).equals(approval("entry_version"))),
		)
		.and_where(Expr::col(registry("kind")).eq("model"))
		.and_where(live("authorization_catalog", "node"))
		.order_by(approval("tenant"), Order::Asc)
		.order_by(approval("entry_id"), Order::Asc)
		.order_by(approval("entry_version"), Order::Asc)
		.order_by(approval("enabled"), Order::Desc)
		.order_by(approval("node"), Order::Asc)
		.to_owned();
	let mut targets = Query::select();
	for field in fields {
		match field {
			"node" => {
				targets.column(col("nodes", "name"));
			}
			"scope" => {
				targets.expr(Expr::value("deployment"));
			}
			"revision" => {
				targets.expr(Expr::cust(r#"COALESCE("previous"."revision", 0) + 1"#));
			}
			"restriction" => {
				// SQL JSON array transformation is not available as a typed Query expression.
				targets.expr(Expr::cust(r#"CASE WHEN jsonb_typeof("choice"."restriction"->'subjects') = 'array' THEN jsonb_set("choice"."restriction", '{subjects}', COALESCE((SELECT jsonb_agg(CASE WHEN s LIKE 'aidash://%/' || "choice"."node" || '/agents/%' THEN replace(s, '/' || "choice"."node" || '/agents/', '/' || "nodes"."name" || '/agents/') ELSE s END ORDER BY s) FROM jsonb_array_elements_text("choice"."restriction"->'subjects') AS subjects(s)), '[]'::jsonb)) ELSE "choice"."restriction" END"#));
			}
			other => {
				targets.column(col("choice", other));
			}
		}
	}
	targets.from_subquery(source, Alias::new("choice")).join(
		JoinType::InnerJoin,
		Alias::new("nodes"),
		Expr::col(col("nodes", "tenant")).equals(col("choice", "tenant")),
	);
	targets.and_where(live("nodes", "name"));
	targets.join(
		JoinType::LeftJoin,
		TableRef::table_alias("authorization_catalog", "previous"),
		Expr::col(col("previous", "tenant"))
			.equals(col("choice", "tenant"))
			.and(Expr::col(col("previous", "node")).equals(col("nodes", "name")))
			.and(Expr::col(col("previous", "entry_id")).equals(col("choice", "entry_id")))
			.and(
				Expr::col(col("previous", "entry_version")).equals(col("choice", "entry_version")),
			),
	);
	let upsert = Query::insert()
		.into_table(Alias::new("authorization_catalog"))
		.columns(fields.map(Alias::new))
		.from_subquery(targets.to_owned())
		.on_conflict(
			OnConflict::columns(keys.map(Alias::new))
				.update_columns(
					[
						"enabled",
						"carry_over",
						"scope",
						"restriction",
						"carried_from_version",
						"carried_from_approver",
						"revision",
					]
					.map(Alias::new),
				)
				.to_owned(),
		)
		.returning(fields.map(Alias::new))
		.to_string(PostgresQueryBuilder);
	let history = Query::insert()
		.into_table(Alias::new("authorization_catalog_history"))
		.columns(fields.into_iter().chain(["actor"]).map(Alias::new))
		.from_subquery(
			Query::select()
				.columns(fields.map(Alias::new))
				.expr(Expr::value("migration:identity/0116_model_approval_scope"))
				.from(Alias::new("widened"))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	// Query supports RETURNING but does not express a modifying CTE.
	format!(
		"SET LOCAL aidash.scope = 'deployment';\nLOCK TABLE authorization_catalog IN SHARE ROW EXCLUSIVE MODE;\nWITH widened AS (\n{upsert}\n)\n{history};\n"
	)
}
