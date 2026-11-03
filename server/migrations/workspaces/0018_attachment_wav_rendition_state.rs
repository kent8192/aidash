// reinhardt-migration-source: 1
// Record the cached rendition fields in the ORM state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0018_attachment_wav_rendition_state", "workspaces")
		.add_dependency("workspaces", "0017_attachment_wav_rendition")
		.state_only(true)
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
}
