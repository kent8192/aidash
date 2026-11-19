// reinhardt-migration-source: 1
// A Run a Duty started may set up Duties like any other (#1012), so nothing
// marks a generation request it made any more (0132).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0147_unscheduled_generations", "execution")
		.add_dependency("execution", "0146_stints_row_level_security")
		.add_operation(Operation::DropColumn {
			table: "generation_requests".into(),
			column: "scheduled".into(),
			old_definition: Some(
				ColumnDefinition::new("scheduled", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
			),
		})
		.atomic(true)
}
