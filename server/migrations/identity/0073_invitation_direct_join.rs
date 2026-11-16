// reinhardt-migration-source: 1
// Existing links retain the pending-request behavior.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0073_invitation_direct_join", "identity")
		.add_dependency("identity", "0072_remote_execution_activation")
		.add_operation(Operation::AddColumn {
			table: "tenant_invitations".into(),
			column: ColumnDefinition::new("join_directly", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_invitations".into(),
			column: ColumnDefinition::new("issued_by_mapping", FieldType::Uuid)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_invitations".into(),
			column: ColumnDefinition::new("issued_by_token_hash", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_administration_history".into(),
			column: ColumnDefinition::new("groups", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
		.atomic(true)
}
