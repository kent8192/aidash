// reinhardt-migration-source: 1
// Record the nullable bytea in the ORM state; 0015 changes the physical column.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0016_attachment_content_state", "workspaces")
		.add_dependency("workspaces", "0015_attachment_objects")
		.state_only(true)
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
		.add_operation(Operation::AlterColumn {
			table: "channel_attachments".into(),
			column: "content".into(),
			old_definition: None,
			new_definition: ColumnDefinition::new("content", FieldType::Binary),
			mysql_options: None,
		})
}
