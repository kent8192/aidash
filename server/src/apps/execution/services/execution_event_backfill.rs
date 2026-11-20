//! Stored `run.*` event kinds become `execution.*` (#1009, ADR 0141), as
//! Reinhardt Query output materialized by `migrationseeds` for execution
//! 0150. Journals, the graph and visibility read events back by kind, so the
//! stored rows follow the emitters. Only the prefix changes; the rest of each
//! kind and its data are kept.
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

const DEPLOYMENT_SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";

/// Renames `run.<event>` to `execution.<event>`.
pub(crate) fn forward() -> String {
	let update = Query::update()
		.table(Alias::new("events"))
		.value_expr(
			Alias::new("kind"),
			Expr::cust("'execution.' || substr(\"kind\", 5)"),
		)
		.and_where(Expr::col(Alias::new("kind")).like("run.%"))
		.to_string(PostgresQueryBuilder);
	format!("{DEPLOYMENT_SCOPE}{update};\n")
}

/// Renames `execution.<event>` back to `run.<event>`. Every `execution.*`
/// kind at this migration is a renamed `run.*` one.
pub(crate) fn backward() -> String {
	let update = Query::update()
		.table(Alias::new("events"))
		.value_expr(
			Alias::new("kind"),
			Expr::cust("'run.' || substr(\"kind\", 11)"),
		)
		.and_where(Expr::col(Alias::new("kind")).like("execution.%"))
		.to_string(PostgresQueryBuilder);
	format!("{DEPLOYMENT_SCOPE}{update};\n")
}

#[cfg(test)]
mod tests {
	#[test]
	fn the_event_statements_swap_only_the_prefix() {
		assert!(super::forward().contains("'execution.' || substr(\"kind\", 5)"));
		assert!(super::forward().contains("LIKE 'run.%'"));
		assert!(super::backward().contains("'run.' || substr(\"kind\", 11)"));
		assert!(super::backward().contains("LIKE 'execution.%'"));
	}
}
