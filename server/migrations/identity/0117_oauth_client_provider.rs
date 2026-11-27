// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0117_oauth_client_provider", "identity")
		.add_dependency("identity", "0116_model_approval_scope")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "oauth_clients".into(),
			column: ColumnDefinition::new("provider", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
}
