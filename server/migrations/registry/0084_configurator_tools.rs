// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// Admit the explicitly bound Tenant Configurator tools (#1155).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0084_configurator_tools", "registry")
		.add_dependency("registry", "0083_optional_agent_model")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0084_configurator_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0084_configurator_tools.sql").into()),
		})
}
