// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// Admit the explicitly bound Tenant Configurator tools (#1155).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0085_configurator_review_tool", "registry")
		.add_dependency("registry", "0084_configurator_tools")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0085_configurator_review_tool.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0085_configurator_review_tool.sql").into(),
			),
		})
}
