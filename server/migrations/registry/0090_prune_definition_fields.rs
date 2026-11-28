// reinhardt-migration-source: 1
// PostgreSQL validator replacement and ALTER TYPE ... USING expressions have
// no typed Reinhardt DDL operation.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0090_prune_definition_fields", "registry")
		.add_dependency("registry", "0082_thread_source_scope")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0090_prune_definition_fields.sql").into(),
			reverse_sql: None,
		})
}
