// reinhardt-migration-source: 1
// Research is no longer a category (#987, ADR 0138): Approval Rules lose the
// `web_search` and `page_reading` kinds, and Catalog Approvals of the removed
// Research definitions go. Deleting them is SQL; the check is typed.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0084_remove_research_approvals", "identity")
		.add_dependency("identity", "0083_data_protection_switches_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "identity_approval_rules".into(),
			constraint: Constraint::Check {
				name: "identity_approval_rules_values".into(),
				expression: "kind IN ('tool_call', 'outbound', 'web_search', 'page_reading', 'generation') AND effect IN ('allow', 'ask', 'deny') AND jsonb_typeof(target) IN ('string', 'object')".into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0084_remove_research_approvals.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0084_remove_research_approvals.sql").into()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "identity_approval_rules".into(),
			constraint: Constraint::Check {
				name: "identity_approval_rules_values".into(),
				expression: "kind IN ('tool_call', 'outbound', 'generation') AND effect IN ('allow', 'ask', 'deny') AND jsonb_typeof(target) IN ('string', 'object')".into(),
			},
		})
}
