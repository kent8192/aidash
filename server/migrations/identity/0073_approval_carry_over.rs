// reinhardt-migration-source: 1
// Existing approvals remain Node-scoped and do not carry (ADR 0128).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	let mut migration = Migration::new("0073_approval_carry_over", "identity")
		.add_dependency("identity", "0072_remote_execution_activation")
		.database_only(true)
		.atomic(true);
	for table in ["authorization_catalog", "authorization_catalog_history"] {
		for column in [
			ColumnDefinition::new("carry_over", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			ColumnDefinition::new("scope", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("'node'".into())),
			ColumnDefinition::new("carried_from_version", FieldType::Text).with_not_null(false),
			ColumnDefinition::new("carried_from_approver", FieldType::Text).with_not_null(false),
		] {
			migration = migration.add_operation(Operation::AddColumn {
				table: table.into(),
				column,
				mysql_options: None,
			});
		}
	}
	migration
}
