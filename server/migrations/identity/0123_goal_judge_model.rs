// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0123_goal_judge_model", "identity")
		.add_dependency("identity", "0115_suspension_resume_refusal")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tenants".into(),
			column: ColumnDefinition::new("goal_judge_model", FieldType::Jsonb),
			mysql_options: None,
		})
}
