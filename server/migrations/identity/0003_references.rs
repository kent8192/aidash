// reinhardt-migration-source: 1
// Generated baseline: PostgreSQL schema and seed data captured from the old history.
// Raw SQL preserves physical PostgreSQL DDL and data alongside the typed state snapshot.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_references", "identity")
		.database_only(true)
		.add_dependency("execution", "0002_tables")
		.add_dependency("federation", "0002_tables")
		.add_dependency("identity", "0002_tables")
		.add_dependency("knowledge", "0002_tables")
		.add_dependency("marketplace", "0002_tables")
		.add_dependency("operations", "0002_tables")
		.add_dependency("registry", "0002_tables")
		.add_dependency("workspaces", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0003_references.sql").to_owned(),
			reverse_sql: Some("DO $$ BEGIN RAISE EXCEPTION 'Aidash baseline is irreversible; recreate the database'; END $$;".to_owned()),
		})
}
