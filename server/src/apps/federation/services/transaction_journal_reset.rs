//! The journal reset of `federation/0031_transaction_parts` (#1107, ADR 0151),
//! materialized by `migrationseeds`.
//!
//! From #1107 a coordinator seals every part with a per-transaction seed and a
//! participant keeps only its own share. Journals in the former shape carry no
//! seed and hold whole manifests, so no build can read them. Aidash is
//! undeployed and new cross-Deployment submissions were refused while the old
//! shape was current, so they are discarded before `atomic_coordinators.seed`
//! becomes NOT NULL. Reinhardt migrations have no operation that changes rows.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};

/// Journal tables in delete order: every table that names a transaction
/// before `atomic_coordinators`, which the others follow.
const JOURNAL: [&str; 7] = [
	"atomic_authority_attempts",
	"atomic_preflights",
	"atomic_subjects",
	"atomic_history",
	"atomic_votes",
	"atomic_participants",
	"atomic_coordinators",
];

/// Release the gate first, then empty the journal, coordinators last.
pub(crate) fn forward() -> String {
	let gate = Query::update()
		.table(Alias::new("atomic_gate"))
		.value(Alias::new("transaction_id"), None::<String>)
		.and_where(Expr::col(Alias::new("transaction_id")).is_not_null())
		.to_string(PostgresQueryBuilder);
	let mut sql = format!("SET LOCAL search_path = public, pg_catalog;\n\n{gate};\n");
	for table in JOURNAL {
		let delete = Query::delete()
			.from_table(Alias::new(table))
			.to_string(PostgresQueryBuilder);
		sql.push_str(&format!("{delete};\n"));
	}
	sql
}
