//! The deletes of `execution/0141_remove_research` (#987, ADR 0138),
//! materialized by `migrationseeds`.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

/// Approval kinds observed for Web disclosure approvals. Observation reports
/// no longer decode them.
const DISCLOSURE_KINDS: [&str; 2] = ["web_search", "page_reading"];

/// Web disclosure approvals were the only `disclosure` records. Nothing reads
/// them any more, so a pending one would never be answered; they are deleted,
/// and a Run still waiting on one must be cancelled. Their observations go
/// first, since one row of a removed kind would fail every report that spans
/// it; deleting an observation cascades to its attempts.
pub(crate) fn forward() -> String {
	let disclosures = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("core_records"))
		.and_where(Expr::col(Alias::new("kind")).eq("disclosure"))
		.take();
	let population = Query::select()
		.column(Alias::new("approval"))
		.from(Alias::new("approval_observation_population"))
		.and_where(Expr::col(Alias::new("kind")).is_in(DISCLOSURE_KINDS))
		.take();
	let observations = Query::delete()
		.from_table(Alias::new("approval_observations"))
		.and_where(Expr::col(Alias::new("approval_type")).eq("capability"))
		.and_where(
			Expr::col(Alias::new("approval"))
				.in_subquery(disclosures)
				.or(Expr::col(Alias::new("approval")).in_subquery(population)),
		)
		.to_string(PostgresQueryBuilder);
	let population = Query::delete()
		.from_table(Alias::new("approval_observation_population"))
		.and_where(Expr::col(Alias::new("kind")).is_in(DISCLOSURE_KINDS))
		.to_string(PostgresQueryBuilder);
	let records = Query::delete()
		.from_table(Alias::new("core_records"))
		.and_where(Expr::col(Alias::new("kind")).eq("disclosure"))
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{observations};\n{population};\n{records};\n")
}

/// The deleted records are not restored.
pub(crate) fn backward() -> String {
	format!(
		"{};\n",
		Query::select()
			.expr(Expr::value(1))
			.to_string(PostgresQueryBuilder)
	)
}
