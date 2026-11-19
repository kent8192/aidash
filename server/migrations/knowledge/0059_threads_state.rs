// reinhardt-migration-source: 1
// ORM state of the constraint names 0058 renamed in SQL (#1008); the database
// already carries them.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0059_threads_state", "knowledge")
		.add_dependency("knowledge", "0058_threads")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_banks".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_bank_workspace".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_participants".to_string(),
			constraint: Constraint::ForeignKey {
				name: "memory_participant_workspace".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_collections".to_string(),
			constraint: Constraint::ForeignKey {
				name: "semantic_collections_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "semantic_indexes".to_string(),
				referenced_columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_entries".to_string(),
			constraint: Constraint::ForeignKey {
				name: "semantic_entries_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "semantic_indexes".to_string(),
				referenced_columns: vec!["tenant".to_string(), "workspace_id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_image_publications".to_string(),
			constraint: Constraint::ForeignKey {
				name: "semantic_image_publications_workspace_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraint {
			table: "memory_banks".to_string(),
			constraint_sql: "CONSTRAINT memory_bank_thread FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "memory_participants".to_string(),
			constraint_sql: "CONSTRAINT memory_participant_thread FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "semantic_collections".to_string(),
			constraint_sql: "CONSTRAINT semantic_collections_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES semantic_indexes(tenant, thread_id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "semantic_entries".to_string(),
			constraint_sql: "CONSTRAINT semantic_entries_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES semantic_indexes(tenant, thread_id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "semantic_image_publications".to_string(),
			constraint_sql: "CONSTRAINT semantic_image_publications_thread_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE CASCADE ON UPDATE NO ACTION"
				.to_string(),
		})
}
