// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed Reinhardt operation. This DDL-only
// exception extends the 0034 descriptor guard and the 0042 transport guard for
// connection_request, secret_owner and MCP scopes. Rollback restores both exact
// prior bodies without changing Registry data.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0053_connection_request", "registry")
		.add_dependency("registry", "0052_harness_contract_structured3")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0053_connection_request.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0053_connection_request.sql").into()),
		})
}
