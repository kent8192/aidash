// reinhardt-migration-source: 1
// Model state for the foreign keys that `#[rel(foreign_key)]` fields declare.
// The constraints already exist in PostgreSQL, so this migration is state-only.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0044_foreign_key_relations_state", "registry")
		.add_dependency("registry", "0043_external_services_state")
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_incident_events".to_string(),
			constraint: Constraint::ForeignKey {
				name: "agent_incident_events_incident_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "incident_id".to_string()],
				referenced_table: "agent_incidents".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_test_sessions".to_string(),
			constraint: Constraint::ForeignKey {
				name: "agent_test_sessions_draft_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "draft_id".to_string()],
				referenced_table: "agent_drafts".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
