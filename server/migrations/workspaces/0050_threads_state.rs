// reinhardt-migration-source: 1
// ORM state of the removed reply-thread table; 0049 changed the database.
// The ORM state also takes the constraint names 0049 renamed in SQL, and drops
// the Thread content CHECK 0049 rebuilt: the models never declared it.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_threads_state", "workspaces")
		.add_dependency("workspaces", "0049_threads")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "channel_threads".into(),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "artifacts".to_string(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "channel_attachments".to_string(),
			constraint: Constraint::ForeignKey {
				name: "channel_attachments_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "conversations".to_string(),
			constraint: Constraint::ForeignKey {
				name: "conversations_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "messages".to_string(),
			constraint: Constraint::ForeignKey {
				name: "messages_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "tasks".to_string(),
			constraint: Constraint::ForeignKey {
				name: "tasks_workspace_id_fkey".to_string(),
				columns: vec!["tenant".to_string(), "thread_id".to_string()],
				referenced_table: "threads".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "threads".to_string(),
			constraint: Constraint::Check {
				name: "threads_content".to_string(),
				expression: "COALESCE(((length(btrim(title, E'\\u0009\\u000a\\u000b\\u000c\\u000d \\u0085\\u00a0\\u1680\\u2000\\u2001\\u2002\\u2003\\u2004\\u2005\\u2006\\u2007\\u2008\\u2009\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000'::text)) > 0) AND (revision >= 0)), false)"
					.to_string(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "threads".to_string(),
			constraint: Constraint::ForeignKey {
				name: "workspaces_scratch_space_fkey".to_string(),
				columns: vec!["tenant".to_string(), "scratch_space_id".to_string()],
				referenced_table: "scratch_spaces".to_string(),
				referenced_columns: vec!["tenant".to_string(), "id".to_string()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraint {
			table: "artifacts".to_string(),
			constraint_sql: "CONSTRAINT artifacts_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "channel_attachments".to_string(),
			constraint_sql: "CONSTRAINT channel_attachments_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE CASCADE ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "conversations".to_string(),
			constraint_sql: "CONSTRAINT conversations_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "messages".to_string(),
			constraint_sql: "CONSTRAINT messages_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "tasks".to_string(),
			constraint_sql: "CONSTRAINT tasks_thread_id_fkey FOREIGN KEY (tenant, thread_id) REFERENCES threads(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
		.add_operation(Operation::AddConstraint {
			table: "threads".to_string(),
			constraint_sql: "CONSTRAINT threads_scratch_space_fkey FOREIGN KEY (tenant, scratch_space_id) REFERENCES scratch_spaces(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
				.to_string(),
		})
}
