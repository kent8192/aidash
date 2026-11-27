// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// Admit the Configurator's Scratch Space settings tool (#1155).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0086_configurator_scratch_tool", "registry")
		.add_dependency("registry", "0085_configurator_review_tool")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0086_configurator_scratch_tool.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0086_configurator_scratch_tool.sql").into(),
			),
		})
}
