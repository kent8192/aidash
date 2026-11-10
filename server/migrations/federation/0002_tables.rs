// reinhardt-migration-source: 1
// Generated baseline: PostgreSQL schema and seed data captured from the old history.
// Raw SQL preserves physical PostgreSQL DDL and data alongside the typed state snapshot.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "federation")
		.database_only(true)
		.add_dependency("federation", "0001_state")
		.add_dependency("operations", "0000_environment")
		.add_dependency("operations", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0002_tables.sql").to_owned(),
			reverse_sql: Some("DO $$ BEGIN RAISE EXCEPTION 'Aidash baseline is irreversible; recreate the database'; END $$;".to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_seed.sql").to_owned(),
			reverse_sql: Some("DO $$ BEGIN RAISE EXCEPTION 'Aidash baseline is irreversible; recreate the database'; END $$;".to_owned()),
		})
}
