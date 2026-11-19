// reinhardt-migration-source: 1
// The Workspace is dissolved into Threads (#1008, ADR 0140): the `workspaces`
// table becomes `threads` (same ids) without its goal and state, every
// `workspace_id` of this app becomes `thread_id`, and reply threads go. Aidash
// is undeployed, so reply threads are dropped, not converted: their messages
// stay as flat messages of their Thread, and the columns that named a reply
// thread are dropped. A Duty keeps the Thread it was registered in as its own
// and gains the Thread it was registered from (NULL for these). Constraint and
// index renames, the dependency trigger body and the forced-policy table drop
// are SQL: Reinhardt has no typed operation for them. Deleting a Thread marks
// it (`deleted_at`) so every read and write refuses it. Reversal restores the
// schema only: empty reply threads, and a goal copied from each title.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0049_threads", "workspaces")
		.add_dependency("workspaces", "0047_duty_creating_task")
		// Every app's history up to here may name the `workspaces` table.
		.add_dependency("execution", "0147_unscheduled_generations")
		.add_dependency("identity", "0109_agent_created_nodes")
		.add_dependency("knowledge", "0057_journal_proofs_without_research_state")
		.add_dependency("federation", "0023_delegation_unused_columns_state")
		.add_dependency("registry", "0080_node_tools")
		.add_dependency("marketplace", "0011_tenant_owned_row_level_security")
		.add_dependency("operations", "0007_model_state")
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "channel_message_context".into(),
			constraint: Constraint::ForeignKey {
				name: "channel_message_context_workspace_id_thread_id_fkey".into(),
				columns: vec!["tenant".into(), "workspace_id".into(), "thread_id".into()],
				referenced_table: "channel_threads".into(),
				referenced_columns: vec!["tenant".into(), "workspace_id".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropNamedIndex {
			table: "channel_message_context".into(),
			name: "channel_reply_lookup".into(),
			columns: vec!["workspace_id".into(), "thread_id".into(), "message_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::DropColumn {
			table: "channel_message_context".into(),
			column: "thread_id".into(),
			old_definition: Some(ColumnDefinition::new("thread_id", FieldType::Uuid)),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0049_drop_channel_threads.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0049_drop_channel_threads.sql").into()),
		})
		.add_operation(Operation::DropColumn {
			table: "duties".into(),
			column: "thread_id".into(),
			old_definition: Some(ColumnDefinition::new("thread_id", FieldType::Uuid)),
		})
		.add_operation(Operation::DropColumn {
			table: "duty_ticks".into(),
			column: "thread_id".into(),
			old_definition: Some(ColumnDefinition::new("thread_id", FieldType::Uuid)),
		})
		.add_operation(Operation::RenameColumn {
			table: "artifacts".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "channel_attachments".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "channel_message_context".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "conversations".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duties".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duty_ticks".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duty_triggers".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duty_events".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duty_bookings".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "messages".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "task_dependencies".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "tasks".into(),
			old_name: "workspace_id".into(),
			new_name: "thread_id".into(),
		})
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("origin_thread_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "workspaces".into(),
			constraint: Constraint::Check {
				name: "workspaces_content".into(),
				expression: r#"COALESCE(((length(btrim(title, E'\u0009\u000a\u000b\u000c\u000d \u0085                 　'::text)) > 0) AND (length(btrim(goal, E'\u0009\u000a\u000b\u000c\u000d \u0085                 　'::text)) > 0) AND (jsonb_typeof(state) = 'object'::text) AND (revision >= 0)), false)"#.into(),
			},
		})
		// Backward only: give each restored goal its title, then drop the
		// default below that lets the NOT NULL goal come back.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0049_workspace_goal.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0049_workspace_goal.sql").into()),
		})
		.add_operation(Operation::DropColumn {
			table: "workspaces".into(),
			column: "goal".into(),
			old_definition: Some(
				ColumnDefinition::new("goal", FieldType::Text)
					.with_not_null(true)
					.with_default(Some("''".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "workspaces".into(),
			column: "state".into(),
			old_definition: Some(
				ColumnDefinition::new("state", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some("'{}'::jsonb".into())),
			),
		})
		.add_operation(Operation::RenameTable {
			old_name: "workspaces".into(),
			new_name: "threads".into(),
		})
		// Deleting a Thread marks it; every read and write refuses it then.
		.add_operation(Operation::AddColumn {
			table: "threads".into(),
			column: ColumnDefinition::new("deleted_at", FieldType::TimestampTz),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "threads".into(),
			constraint: Constraint::Check {
				name: "threads_content".into(),
				expression: r#"COALESCE(((length(btrim(title, E'\u0009\u000a\u000b\u000c\u000d \u0085                 　'::text)) > 0) AND (revision >= 0)), false)"#.into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0049_thread_names.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0049_thread_names.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0049_thread_dependencies.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0049_thread_dependencies.sql").into()),
		})
}
