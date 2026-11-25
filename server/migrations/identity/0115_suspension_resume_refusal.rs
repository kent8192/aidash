// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0115_suspension_resume_refusal", "identity")
		.add_dependency("identity", "0114_policy_revision_backfill")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "dashboard_execution_origins".into(),
			column: ColumnDefinition::new("suspension_resume_refusal", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
}
