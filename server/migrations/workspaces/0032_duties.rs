// reinhardt-migration-source: 1
// Schedules become Duties (#1011, ADR 0142): the tables and the Requester and
// mission columns take their new names, a Duty gains a procedure and the
// number of failed Tasks in a row after which it pauses (3 for every existing
// one), and the time trigger becomes optional.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0032_duties", "workspaces")
		.add_dependency("workspaces", "0031_schedules_model_state")
		.atomic(true)
		.add_operation(Operation::RenameTable {
			old_name: "schedules".into(),
			new_name: "duties".into(),
		})
		.add_operation(Operation::RenameTable {
			old_name: "schedule_ticks".into(),
			new_name: "duty_ticks".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duties".into(),
			old_name: "registrant".into(),
			new_name: "requester".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duties".into(),
			old_name: "request".into(),
			new_name: "mission".into(),
		})
		.add_operation(Operation::RenameColumn {
			table: "duty_ticks".into(),
			old_name: "schedule_id".into(),
			new_name: "duty_id".into(),
		})
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("procedure", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("''".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "duties".into(),
			column: "procedure".into(),
			old_definition: Some(
				ColumnDefinition::new("procedure", FieldType::Text)
					.with_not_null(true)
					.with_default(Some("''".into())),
			),
			new_definition: ColumnDefinition::new("procedure", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("pause_after_failures", FieldType::Integer)
				.with_not_null(true)
				.with_default(Some("3".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "duties".into(),
			column: "pause_after_failures".into(),
			old_definition: Some(
				ColumnDefinition::new("pause_after_failures", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("3".into())),
			),
			new_definition: ColumnDefinition::new("pause_after_failures", FieldType::Integer)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "duties".into(),
			column: "trigger".into(),
			old_definition: Some(
				ColumnDefinition::new("trigger", FieldType::Jsonb).with_not_null(true),
			),
			new_definition: ColumnDefinition::new("trigger", FieldType::Jsonb),
			mysql_options: None,
		})
}
