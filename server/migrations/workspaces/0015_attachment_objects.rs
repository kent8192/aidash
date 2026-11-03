// reinhardt-migration-source: 1
// Allow a staged transfer of attachment bytes to the immutable object store.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0015_attachment_objects", "workspaces")
		.add_dependency("workspaces", "0014_remote_file_version_citations")
		.add_dependency("execution", "0061_retained_versions")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("object_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("width", FieldType::Integer),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("height", FieldType::Integer),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("duration_ms", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".into(),
			constraint: Constraint::ForeignKey {
				name: "channel_attachments_object_fkey".into(),
				columns: vec!["tenant".into(), "object_id".into()],
				referenced_table: "core_objects".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".into(),
			constraint: Constraint::Check {
				name: "channel_attachments_has_content".into(),
				expression: "object_id IS NOT NULL OR content IS NOT NULL".into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0015_attachment_objects.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0015_attachment_objects.sql").to_owned()),
		})
}
