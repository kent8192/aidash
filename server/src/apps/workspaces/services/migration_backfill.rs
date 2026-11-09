//! Query-generated backfill for conversations created before Scratch Spaces.
use reinhardt::query::{
	Alias, Expr, JoinType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
/// The settings these backfills gave each Scratch Space when migrations 0021 and
/// 0024 were written. Applied migrations must not change, so this stays fixed
/// when `ScratchSettings::default()` does (#729 later added five default hosts).
pub fn legacy_settings() -> serde_json::Value {
	serde_json::json!({
		"allowed_hosts": {
			"hosts": [
				"archive.ubuntu.com",
				"auth.docker.io",
				"crates.io",
				"deb.debian.org",
				"files.pythonhosted.org",
				"ghcr.io",
				"github.com",
				"index.crates.io",
				"objects.githubusercontent.com",
				"proxy.golang.org",
				"pypi.org",
				"quay.io",
				"registry-1.docker.io",
				"registry.npmjs.org",
				"security.ubuntu.com",
				"static.crates.io",
				"sum.golang.org"
			],
			"kind": "hosts"
		},
		"capacity_bytes": 107374182400u64,
		"cpu_millis": 4000,
		"daily_received_bytes": 107374182400u64,
		"daily_sent_bytes": 107374182400u64,
		"docker_enabled": false,
		"execution_image": null,
		"external_services": [],
		"idle_expiry_days": 14,
		"memory_bytes": 17179869184u64
	})
}

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
		.expr(Expr::value(legacy_settings()))
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
