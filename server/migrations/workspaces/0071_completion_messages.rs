// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0071_completion_messages", "workspaces")
		.add_dependency("workspaces", "0070_task_goals")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "messages".into(),
			column: ColumnDefinition::new("completion", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("planned_completion", FieldType::Uuid),
			mysql_options: None,
		})
		// DropTable reconstructs only the ORM state on reversal. Record the
		// remaining physical schema explicitly so earlier migrations can reverse.
		.add_operation(Operation::RunSQL {
			// Reinhardt has no typed operation for PostgreSQL policies or triggers.
			sql: include_str!("sql/forward/0071_artifact_guards.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0071_artifact_guards.sql").into()),
		})
		.add_operation(Operation::AlterColumn {
			table: "artifacts".into(),
			column: "created_at".into(),
			old_definition: Some(
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".into())),
			),
			new_definition: ColumnDefinition::new("created_at", FieldType::TimestampTz)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "artifacts".into(),
			constraint: Constraint::Unique {
				name: "artifacts_idempotency_key_key".into(),
				columns: vec!["idempotency_key".into()],
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "artifacts".into(),
			constraint: Constraint::Check {
				name: "artifacts_kind_check".into(),
				expression: "(kind = ANY (ARRAY['text'::text, 'json'::text, 'file_reference'::text, 'code'::text, 'structured_result'::text]))".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "artifacts".into(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_task_thread".into(),
				columns: vec!["tenant".into(), "task_id".into(), "thread_id".into()],
				referenced_table: "tasks".into(),
				referenced_columns: vec!["tenant".into(), "id".into(), "thread_id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "artifacts".into(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_thread_id_fkey".into(),
				columns: vec!["tenant".into(), "thread_id".into()],
				referenced_table: "threads".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "artifacts".into(),
			constraint: Constraint::ForeignKey {
				name: "artifacts_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropTable {
			name: "artifacts".into(),
		})
}
