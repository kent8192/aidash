// reinhardt-migration-source: 1
// Issue #296: HTTP and MCP tool transports no longer name Deployment secrets.
// The transport contract is a PostgreSQL function body, which has no typed
// Reinhardt operation; rollback restores the registry/0001 body. The registry
// checks are NOT VALID for existing rows, and the server refuses a stored
// transport that still carries credential_env when it dispatches the Tool.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0028_tool_transport_secrets", "registry")
		.add_dependency("registry", "0027_remove_legacy_installations_state")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0028_tool_transport_secrets.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0028_tool_transport_secrets.sql").into()),
		})
}
