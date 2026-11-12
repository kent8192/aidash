// reinhardt-migration-source: 1
// An image Thread Attachment may carry its writer's Image Description (#622).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0027_attachment_description", "workspaces")
		.add_dependency("workspaces", "0026_scratch_binding_indexes")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "channel_attachments".into(),
			column: ColumnDefinition::new("description", FieldType::Text),
			mysql_options: None,
		})
}
