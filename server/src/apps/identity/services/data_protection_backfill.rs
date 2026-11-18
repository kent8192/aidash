//! The stored policy rewrites of `identity/0083_data_protection_switches`
//! (#962, #963, ADR 0137), materialized by `migrationseeds`. Both directions
//! run while the migration's old and new columns exist side by side.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

const CURRENT: &str = "tenant_data_protection";
const HISTORY: &str = "tenant_data_protection_history";

fn statements(statements: Vec<String>) -> String {
	format!(
		"SET LOCAL aidash.scope = 'deployment';\n{};\n",
		statements.join(";\n")
	)
}

/// Every existing Tenant keeps its level (#963 decision 5). A Tenant that
/// never saved a policy read as Standard, so it gets that policy as a row
/// first; without one it would read as a new Tenant's No Training. A Tenant
/// that had turned the Plan Connection exemption off above Standard could not
/// use Plan Connections, so it starts with them forbidden. Keyless endpoints
/// start allowed, as #962 decides for every Tenant. Standard is renamed.
pub(crate) fn forward() -> String {
	let saved = Query::select()
		.expr(Expr::value(1))
		.from(Alias::new(CURRENT))
		.and_where(
			Expr::col((Alias::new(CURRENT), Alias::new("tenant")))
				.equals((Alias::new("tenants"), Alias::new("name"))),
		)
		.take();
	let implicit = Query::select()
		.column(Alias::new("name"))
		.expr(Expr::value("standard"))
		.expr(Expr::value(true))
		.expr(Expr::value(true))
		.expr(Expr::cust("CURRENT_TIMESTAMP"))
		.expr(Expr::value("migration"))
		.from(Alias::new("tenants"))
		.and_where(Expr::not_exists(saved))
		.take();
	let mut rewrites = vec![
		Query::insert()
			.into_table(Alias::new(CURRENT))
			.columns(
				[
					"tenant",
					"level",
					"model_call_zdr",
					"plan_connection_exempt",
					"updated_at",
					"updated_by",
				]
				.map(Alias::new),
			)
			.from_subquery(implicit)
			.to_string(PostgresQueryBuilder),
	];
	for table in [CURRENT, HISTORY] {
		rewrites.push(
			Query::update()
				.table(Alias::new(table))
				.value_expr(
					Alias::new("forbid_plan_connections"),
					Expr::col(Alias::new("plan_connection_exempt"))
						.not()
						.and(Expr::col(Alias::new("level")).ne("standard")),
				)
				.to_string(PostgresQueryBuilder),
		);
	}
	for table in [CURRENT, HISTORY] {
		rewrites.push(renamed(table, "standard", "no_restriction"));
	}
	statements(rewrites)
}

/// Restores the former levels: Safety Retention did not exist, so it becomes
/// Zero Data Retention, the nearest level that is not weaker. The rows added
/// for Tenants that had none stay; they hold the former default policy.
pub(crate) fn backward() -> String {
	let mut rewrites = Vec::new();
	for table in [CURRENT, HISTORY] {
		rewrites.push(
			Query::update()
				.table(Alias::new(table))
				.value_expr(
					Alias::new("plan_connection_exempt"),
					Expr::col(Alias::new("forbid_plan_connections")).not(),
				)
				.value(Alias::new("model_call_zdr"), true)
				.to_string(PostgresQueryBuilder),
		);
	}
	for (from, to) in [
		("no_restriction", "standard"),
		("safety_retention", "zero_data_retention"),
	] {
		for table in [CURRENT, HISTORY] {
			rewrites.push(renamed(table, from, to));
		}
	}
	statements(rewrites)
}

fn renamed(table: &str, from: &str, to: &str) -> String {
	Query::update()
		.table(Alias::new(table))
		.value(Alias::new("level"), to)
		.and_where(Expr::col(Alias::new("level")).eq(from))
		.to_string(PostgresQueryBuilder)
}
