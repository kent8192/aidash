// reinhardt-migration-source: 1
// The historical CHECK is database-only; 0085 tracks the typed ModelEnum separately.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0084_mcp_connection_requests", "execution")
        .add_dependency("execution", "0083_invocation_connections")
        .atomic(true).database_only(true)
        .add_operation(Operation::DropConstraintDefinition { table: "human_requests".into(), constraint: Constraint::Check { name: "human_requests_kind_check".into(), expression: "kind IN ('QUESTION','APPROVAL_REQUIRED','CONFIRMATION','INFORMATION_REQUEST','MCP_ELICITATION')".into() } })
        .add_operation(Operation::AddConstraintDefinition { table: "human_requests".into(), constraint: Constraint::Check { name: "human_requests_kind_check".into(), expression: "kind IN ('QUESTION','APPROVAL_REQUIRED','CONFIRMATION','INFORMATION_REQUEST','MCP_ELICITATION','MCP_CONNECTION')".into() } })
}
