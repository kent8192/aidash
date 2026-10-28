// reinhardt-migration-source: 1
// Keeps a Run's control provenance true for every writer of `control` (#485,
// ADR 0101). Reinhardt has no trigger operation, so the function and trigger
// are the documented raw-SQL exception. A writer that updates
// `control_revision` itself (the ordinary control path and a Safety Stop) is
// left alone; any other writer that changes `control` bumps the revision here
// and is attributed to the system. The physical default source and the revision
// CHECK are kept out of the logical model state, as the model declares neither.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0076_run_control_trigger", "execution")
		.add_dependency("execution", "0075_run_control_provenance")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AlterColumn {
			table: "runs".into(),
			column: "control_source".into(),
			old_definition: Some(
				ColumnDefinition::new("control_source", FieldType::Text).with_not_null(true),
			),
			new_definition: ColumnDefinition::new("control_source", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("'tenant'".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".into(),
			constraint: Constraint::Check {
				name: "runs_control_provenance".into(),
				expression: "control_revision >= 0".into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0076_run_control_trigger.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0076_run_control_trigger.sql").to_owned()),
		})
}
