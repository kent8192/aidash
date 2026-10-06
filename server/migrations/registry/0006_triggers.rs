// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0006_triggers", "registry")
		.database_only(true)
		.add_dependency("marketplace", "0006_triggers")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0006_triggers.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0006_triggers.sql").to_owned()),
		})
}
