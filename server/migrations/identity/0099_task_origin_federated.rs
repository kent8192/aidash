// reinhardt-migration-source: 1
// Each Task origin records whether its creating chain is Federated, so
// admission reads one row instead of walking an unbounded chain (#1012).
// Aidash is undeployed: existing origins read as Local until recreated.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0099_task_origin_federated", "identity")
		.add_dependency("identity", "0098_duty_member_policy")
		.add_operation(Operation::AddColumn {
			table: "authorization_task_origins".into(),
			column: ColumnDefinition::new("federated", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.atomic(true)
}
