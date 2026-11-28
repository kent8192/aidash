//! Query-generated bootstrap of each Node's default Scratch Space.
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};

pub(crate) fn forward() -> String {
	fn a(name: &str) -> Alias {
		Alias::new(name)
	}
	let mut nodes = Query::select();
	nodes
		.expr(Expr::cust("gen_random_uuid()"))
		.columns(["tenant", "name"].map(a))
		.expr(Expr::value("Node Scratch Space"))
		.expr(Expr::value(""))
		.expr(Expr::value("active"))
		.expr(Expr::value(true))
		.expr(Expr::cust("CURRENT_TIMESTAMP"))
		.expr(Expr::cust("CURRENT_TIMESTAMP"))
		.from(a("nodes"));
	let spaces = Query::insert()
		.into_table(a("scratch_spaces"))
		.columns(
			[
				"id",
				"tenant",
				"node",
				"name",
				"creator",
				"state",
				"is_default",
				"created_at",
				"last_used_at",
			]
			.map(a),
		)
		.from_subquery(nodes.to_owned())
		.to_string(PostgresQueryBuilder);
	// Freeze the defaults for this migration, independently of future policy changes.
	let initial: serde_json::Value = serde_json::from_str(r#"{"allowed_hosts":{"hosts":["api.github.com","api.nuget.org","archive.ubuntu.com","auth.docker.io","builds.hex.pm","cdn.quay.io","cloud.r-project.org","codeload.github.com","conda.anaconda.org","crates.io","deb.debian.org","dl-cdn.alpinelinux.org","downloads.gradle.org","files.pythonhosted.org","getcomposer.org","ghcr.io","github.com","hex.pm","index.crates.io","index.rubygems.org","jsr.io","npm.jsr.io","objects.githubusercontent.com","packagist.org","pkg-containers.githubusercontent.com","plugins.gradle.org","ports.ubuntu.com","production.cloudflare.docker.com","proxy.golang.org","pub.dev","pypi.org","quay.io","registry-1.docker.io","registry.npmjs.org","registry.yarnpkg.com","release-assets.githubusercontent.com","releases.astral.sh","repo.hex.pm","repo.maven.apache.org","repo.packagist.org","repo.yarnpkg.com","repo1.maven.org","rubygems.org","security.debian.org","security.ubuntu.com","services.gradle.org","static.crates.io","static.rust-lang.org","sum.golang.org"],"kind":"hosts"},"capacity_bytes":536870912000,"cpu_millis":8000,"docker_enabled":false,"execution_image":null,"external_services":[],"memory_bytes":34359738368}"#)
		.expect("static Scratch Space settings");
	let mut defaults = Query::select();
	defaults
		.columns(["id", "tenant"].map(a))
		.expr(Expr::value(initial))
		.from(a("scratch_spaces"))
		.and_where(Expr::col(a("is_default")));
	let settings = Query::insert()
		.into_table(a("scratch_space_settings"))
		.columns(["space_id", "tenant", "settings"].map(a))
		.from_subquery(defaults.to_owned())
		.to_string(PostgresQueryBuilder);
	let selected = Query::select()
		.column(a("id"))
		.from(a("scratch_spaces"))
		.and_where(
			Expr::col((a("scratch_spaces"), a("tenant"))).equals((a("threads"), a("tenant"))),
		)
		.and_where(Expr::col((a("scratch_spaces"), a("node"))).equals((a("threads"), a("node"))))
		.and_where(Expr::col(a("is_default")))
		.to_owned();
	let bindings = Query::update()
		.table(a("threads"))
		.value_expr(
			a("scratch_space_id"),
			SimpleExpr::SubQuery(None, Box::new(selected)),
		)
		.to_string(PostgresQueryBuilder);
	let retired = Query::update()
		.table(a("scratch_spaces"))
		.value(a("state"), "cleared")
		.value_expr(a("wipe_generation"), Expr::col(a("wipe_generation")).add(1))
		.and_where(Expr::col(a("is_default")).eq(false))
		.to_string(PostgresQueryBuilder);
	let retire_idle_expiry = Query::update()
		.table(a("scratch_space_settings"))
		.value_expr(
			a("settings"),
			Expr::col(a("settings")).sub(Expr::value("idle_expiry_days")),
		)
		.to_string(PostgresQueryBuilder);
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{spaces};\n{settings};\n{bindings};\n{retired};\n{retire_idle_expiry};\n"
	)
}
