// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0004_references", "registry")
		.database_only(true)
		.add_dependency("marketplace", "0004_references")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_draft_registrations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "agent_draft_registrations_draft_id_fkey".to_owned(),
				columns: vec!["draft_id".to_owned()],
				referenced_table: "agent_drafts".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_draft_shares".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "agent_draft_shares_draft_id_fkey".to_owned(),
				columns: vec!["draft_id".to_owned()],
				referenced_table: "agent_drafts".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_incident_events".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "agent_incident_events_incident_id_fkey".to_owned(),
				columns: vec!["incident_id".to_owned()],
				referenced_table: "agent_incidents".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_knowledge".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "agent_knowledge_agent_id_agent_version_fkey".to_owned(),
				columns: vec!["agent_id".to_owned(), "agent_version".to_owned()],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "agent_test_sessions".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "agent_test_sessions_draft_id_fkey".to_owned(),
				columns: vec!["draft_id".to_owned()],
				referenced_table: "agent_drafts".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "installations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "installations_id_version_fkey".to_owned(),
				columns: vec!["id".to_owned(), "version".to_owned()],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry_agent_model_refs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "registry_agent_model_source".to_owned(),
				columns: vec!["agent_id".to_owned(), "agent_version".to_owned()],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Cascade,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry_agent_model_refs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "registry_agent_model_target".to_owned(),
				columns: vec![
					"model_id".to_owned(),
					"model_version".to_owned(),
					"model_kind".to_owned(),
				],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned(), "kind".to_owned()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry_agent_resource_refs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "registry_agent_resource_source".to_owned(),
				columns: vec!["agent_id".to_owned(), "agent_version".to_owned()],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Cascade,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry_agent_resource_refs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "registry_agent_resource_target".to_owned(),
				columns: vec![
					"reference_id".to_owned(),
					"reference_version".to_owned(),
					"required_kind".to_owned(),
				],
				referenced_table: "registry".to_owned(),
				referenced_columns: vec!["id".to_owned(), "version".to_owned(), "kind".to_owned()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
