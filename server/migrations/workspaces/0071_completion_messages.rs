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
		.add_operation(Operation::DropTable {
			name: "artifacts".into(),
		})
}
