// reinhardt-migration-source: 1
// Exact route evidence and observational estimator inputs (#181). Editing the
// current JSONB allowlist and defining PostgreSQL validation functions cannot
// be expressed with typed Reinhardt schema operations. These SQL assets are
// DDL only; registered immutable definitions are never rewritten.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0045_projection_measurement_config", "registry")
		.add_dependency("registry", "0044_foreign_key_relations_state")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0045_projection_measurement_config.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0045_projection_measurement_config.sql").into(),
			),
		})
		.atomic(true)
}
