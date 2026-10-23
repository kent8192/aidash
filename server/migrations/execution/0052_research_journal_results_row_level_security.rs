// reinhardt-migration-source: 1
// Reinhardt has no row-policy operation (reinhardt-web#6770). These policy
// assets follow database::policy and are checked against its template.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new(
		"0052_research_journal_results_row_level_security",
		"execution",
	)
	.add_dependency("execution", "0051_research_journal_results_state")
	.add_dependency("identity", "0028_tenancy_policy_functions")
	.database_only(true)
	.atomic(true)
	.add_operation(Operation::RunSQL {
		sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
		reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
	})
	.add_operation(Operation::RunSQL {
		sql: include_str!("sql/forward/0052_research_journal_results_row_level_security.sql")
			.to_owned(),
		reverse_sql: Some(
			include_str!("sql/backward/0052_research_journal_results_row_level_security.sql")
				.to_owned(),
		),
	})
}
