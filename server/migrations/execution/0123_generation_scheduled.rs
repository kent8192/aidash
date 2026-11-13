// reinhardt-migration-source: 1
// Mark a generation request a Run a Schedule started made, so the Run its
// generated Agent is delegated carries the mark too (#557 decision 7).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0123_generation_scheduled", "execution")
		.add_dependency("execution", "0122_invocation_refused_state")
		.add_operation(Operation::AddColumn {
			table: "generation_requests".into(),
			column: ColumnDefinition::new("scheduled", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.atomic(true)
}
