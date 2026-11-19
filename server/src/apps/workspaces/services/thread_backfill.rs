//! Data the Thread migrations rewrite (#1008, ADR 0140), as Reinhardt Query
//! output materialized by `migrationseeds`. They run before the columns are
//! renamed, so they name `workspace_id`.
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};

const DEPLOYMENT_SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";

/// execution 0148: the Working Area key becomes (home Node, Thread, Agent,
/// owner). Areas that differed only by their reply thread or Root Task leave
/// one per key; their manifests, holds and Live previews cascade.
pub(crate) fn duplicate_areas() -> String {
	let newer = Query::select()
		.expr(Expr::value(1_i32))
		.from_as(Alias::new("core_areas"), Alias::new("newer"))
		.and_where(Expr::cust(
			"newer.tenant = core_areas.tenant AND newer.home_node = core_areas.home_node AND newer.workspace_id = core_areas.workspace_id AND newer.agent_id = core_areas.agent_id AND newer.owner = core_areas.owner AND newer.id > core_areas.id",
		))
		.to_owned();
	format!(
		"{DEPLOYMENT_SCOPE}{};\n",
		Query::delete()
			.from_table(Alias::new("core_areas"))
			.and_where(Expr::exists(newer))
			.to_string(PostgresQueryBuilder)
	)
}

/// identity 0110: Thread Approval Modes named reply threads, which are gone.
pub(crate) fn reply_thread_modes() -> String {
	format!(
		"{DEPLOYMENT_SCOPE}{};\n",
		Query::delete()
			.from_table(Alias::new("identity_thread_approval_modes"))
			.to_string(PostgresQueryBuilder)
	)
}

#[cfg(test)]
mod tests {
	#[test]
	fn the_data_statements_name_the_columns_before_their_rename() {
		assert!(super::duplicate_areas().contains("newer.workspace_id = core_areas.workspace_id"));
		assert!(
			super::reply_thread_modes().contains("DELETE FROM \"identity_thread_approval_modes\"")
		);
	}
}
