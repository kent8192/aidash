// reinhardt-migration-source: 1
// ORM state of the removed Task to reply-thread map; 0148 changed the database.
// The ORM state also takes the constraint names 0148 renamed in SQL, and drops
// the Working Area key 0148 rebuilt: the models never declared it.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0149_threads_state", "execution")
		.add_dependency("execution", "0148_threads")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "core_task_sessions".into(),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_areas".to_string(),
			constraint: Constraint::Unique {
				name: "core_session_identity".to_string(),
				columns: vec![
					"tenant".to_string(),
					"home_node".to_string(),
					"thread_id".to_string(),
					"agent_id".to_string(),
					"owner".to_string(),
				],
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "events".to_string(),
			constraint: Constraint::ForeignKey {
				name: "events_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_embedding_usage".to_string(),
			constraint: Constraint::ForeignKey {
				name: "generation_embedding_usage_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraint {
			table: "events".to_string(),
			constraint_sql: "CONSTRAINT events_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "generation_embedding_usage".to_string(),
			constraint_sql: "CONSTRAINT generation_embedding_usage_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
}
