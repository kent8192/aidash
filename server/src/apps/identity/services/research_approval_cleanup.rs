//! The deletes of `identity/0084_remove_research_approvals` (#987, #962,
//! ADR 0138), materialized by `migrationseeds`.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

/// The removed Research definitions. Their registry rows stay so pinned Agent
/// versions resolve (#952); `registry/0070_remove_research_providers` drops
/// draft Bindings to the same list.
const REMOVED_DEFINITIONS: [&str; 9] = [
	"aidash.research.brave.search",
	"aidash.research.page.find",
	"aidash.research.page.view",
	"aidash.research.fixture.search",
	"aidash.research.fixture.fetch",
	"aidash.research.fixture.find",
	"aidash.research.fixture.view",
	"aidash.bundle.research_brave",
	"aidash.bundle.research_fixture",
];

/// Approval Rules of the `web_search` and `page_reading` kinds judge nothing,
/// and Catalog Approvals of the removed definitions approve nothing. Page
/// reading 2.0.0 is a new definition, approved afresh like any other.
pub(crate) fn forward() -> String {
	let rules = Query::delete()
		.from_table(Alias::new("identity_approval_rules"))
		.and_where(Expr::col(Alias::new("kind")).is_in(["web_search", "page_reading"]))
		.to_string(PostgresQueryBuilder);
	let approvals = Query::delete()
		.from_table(Alias::new("authorization_catalog"))
		.and_where(Expr::col(Alias::new("entry_id")).is_in(REMOVED_DEFINITIONS))
		.to_string(PostgresQueryBuilder);
	format!("SET LOCAL aidash.scope = 'deployment';\n{rules};\n{approvals};\n")
}
