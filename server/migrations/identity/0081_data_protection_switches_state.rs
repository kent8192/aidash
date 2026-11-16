// reinhardt-migration-source: 1
// ORM state of the Tenant Data Protection switches; 0080 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	let mut migration = Migration::new("0081_data_protection_switches_state", "identity")
		.add_dependency("identity", "0080_data_protection_switches")
		.state_only(true)
		.atomic(true);
	for table in ["tenant_data_protection", "tenant_data_protection_history"] {
		for column in ["forbid_plan_connections", "forbid_keyless_endpoints"] {
			migration = migration.add_operation(Operation::AddColumn {
				table: table.into(),
				column: ColumnDefinition::new(column, FieldType::Boolean).with_not_null(true),
				mysql_options: None,
			});
		}
		for column in ["model_call_zdr", "plan_connection_exempt"] {
			migration = migration.add_operation(Operation::DropColumn {
				table: table.into(),
				column: column.into(),
				old_definition: Some(
					ColumnDefinition::new(column, FieldType::Boolean).with_not_null(true),
				),
			});
		}
	}
	migration
}
