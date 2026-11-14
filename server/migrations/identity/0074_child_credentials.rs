// reinhardt-migration-source: 1
// Parent credentials bound the life of a child credential.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_child_credentials", "identity")
		.add_dependency("identity", "0073_credential_lifetime")
		.add_operation(Operation::AddColumn {
			table: "authorization_credentials".into(),
			column: ColumnDefinition::new("parent_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		.atomic(true)
}
