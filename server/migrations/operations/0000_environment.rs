// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0000_environment", "operations")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0000_environment.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0000_environment.sql").to_owned()),
		})
		// Conditional extension creation needs an ownership marker: native
		// CreateExtension cannot reverse a borrowed administrator-owned extension.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/extension_ownership.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/extension_ownership.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
