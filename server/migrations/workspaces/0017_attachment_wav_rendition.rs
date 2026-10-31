// reinhardt-migration-source: 1
// Cache an immutable audio rendition alongside its original attachment.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0017_attachment_wav_rendition", "workspaces")
		.add_dependency("workspaces", "0016_attachment_content_state")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("wav_object_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("wav_sha256", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("wav_size_bytes", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".into(),
			constraint: Constraint::ForeignKey {
				name: "channel_attachments_wav_object_fkey".into(),
				columns: vec!["tenant".into(), "wav_object_id".into()],
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
				name: "channel_attachments_wav_complete".into(),
				expression: "(wav_object_id IS NULL AND wav_sha256 IS NULL AND wav_size_bytes IS NULL) OR (wav_object_id IS NOT NULL AND wav_sha256 IS NOT NULL AND wav_size_bytes IS NOT NULL)".into(),
			},
		})
}
