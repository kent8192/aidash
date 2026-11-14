// reinhardt-migration-source: 1
// PostgreSQL function bodies cannot be expressed by typed migration operations.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0064_journal_tools", "registry")
		.add_dependency("registry", "0063_reference_set_limit")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0064_journal_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0064_journal_tools.sql").into()),
		})
}
