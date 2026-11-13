// reinhardt-migration-source: 1
// Record how each Provider Credential's Key Material reached Aidash (#892):
// Provider Authorization, pasted in the dashboard, or stored through Host
// Access. Existing rows all came from Provider Authorization, so the default
// fills them once and is then dropped, like `inference_usage.purpose`.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0073_provider_credential_origin", "identity")
		.add_dependency("identity", "0072_remote_execution_activation")
		.add_operation(Operation::AddColumn {
			table: "provider_credentials".into(),
			column: ColumnDefinition::new("key_origin", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("'authorization'".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "provider_credentials".into(),
			column: "key_origin".into(),
			old_definition: Some(
				ColumnDefinition::new("key_origin", FieldType::Text)
					.with_not_null(true)
					.with_default(Some("'authorization'".into())),
			),
			new_definition: ColumnDefinition::new("key_origin", FieldType::Text)
				.with_not_null(true),
			mysql_options: None,
		})
		.atomic(true)
}
