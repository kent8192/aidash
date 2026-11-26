// reinhardt-migration-source: 1
// Preserve consent history on reversal.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0116_model_approval_scope", "identity")
		.add_dependency("identity", "0115_suspension_resume_refusal")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0116_model_approval_scope.sql").to_owned(),
			reverse_sql: Some("SELECT 1;\n".to_owned()),
		})
}
