// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0001_functions", "workspaces")
		.database_only(true)
		.add_dependency("registry", "0001_functions")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0001_functions.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0001_functions.sql").to_owned()),
		})
}
