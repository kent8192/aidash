// reinhardt-migration-source: 1
// The existing human-request check is database-only; the model enum follows in 0051.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_mcp_elicitation_kind", "execution")
        .add_dependency("execution", "0049_foreign_key_relations_state")
        .atomic(true).database_only(true)
        .add_operation(Operation::DropConstraintDefinition { table: "human_requests".into(), constraint: Constraint::Check { name: "human_requests_kind_check".into(), expression: "kind IN ('QUESTION','APPROVAL_REQUIRED','CONFIRMATION','INFORMATION_REQUEST')".into() } })
        .add_operation(Operation::AddConstraintDefinition { table: "human_requests".into(), constraint: Constraint::Check { name: "human_requests_kind_check".into(), expression: "kind IN ('QUESTION','APPROVAL_REQUIRED','CONFIRMATION','INFORMATION_REQUEST','MCP_ELICITATION')".into() } })
}
