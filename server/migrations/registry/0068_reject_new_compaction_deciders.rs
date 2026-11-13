// reinhardt-migration-source: 1
// Reinhardt cannot express PostgreSQL's NOT VALID CHECK constraint.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0068_reject_new_compaction_deciders", "registry")
		.add_dependency("registry", "0067_optional_memory_roles")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0068_reject_new_compaction_deciders.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0068_reject_new_compaction_deciders.sql").into(),
			),
		})
}
