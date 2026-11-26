//! Record policy snapshots when a migration advances a Tenant's revision.

use reinhardt::query::{
	Alias, Expr, InsertStatement, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	UpdateStatement,
};

const SNAPSHOT_COLUMNS: [&str; 3] = ["tenant", "revision", "document"];

fn insert_snapshot(source: &str, actor: &str) -> InsertStatement {
	Query::insert()
		.into_table(Alias::new("authorization_revisions"))
		.columns(["tenant", "revision", "document", "actor"].map(Alias::new))
		.from_subquery(
			Query::select()
				.columns(SNAPSHOT_COLUMNS.map(Alias::new))
				.expr(Expr::value(actor))
				.from(Alias::new(source))
				.to_owned(),
		)
		.to_owned()
}

pub(super) fn record_update(update: &mut UpdateStatement, actor: &str) -> String {
	let update = update
		.returning(SNAPSHOT_COLUMNS.map(Alias::new))
		.to_string(PostgresQueryBuilder);
	let insert = insert_snapshot("updated_bundles", actor).to_string(PostgresQueryBuilder);
	// Reinhardt Query builds both statements but cannot use UPDATE as a CTE.
	// PostgreSQL's RETURNING supplies exactly the snapshots this update wrote.
	format!("WITH updated_bundles AS (\n{update}\n)\n{insert}")
}

pub(crate) fn forward() -> String {
	let insert = insert_snapshot(
		"authorization_bundles",
		"migration:identity/0114_policy_revision_backfill",
	)
	.on_conflict(
		OnConflict::columns(["tenant", "revision"].map(Alias::new))
			.do_nothing()
			.to_owned(),
	)
	.to_string(PostgresQueryBuilder);
	format!(
		"SET LOCAL aidash.scope = 'deployment';\nSET LOCAL search_path = public, pg_catalog;\n{insert};\n"
	)
}
