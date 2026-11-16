// reinhardt-migration-source: 1
// ORM state of the Tenant Data Protection switches; 0080 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0081_data_protection_switches_state", "identity")
		.add_dependency("identity", "0080_data_protection_switches")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection".into(),
			column: ColumnDefinition::new("forbid_plan_connections", FieldType::Boolean)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection".into(),
			column: ColumnDefinition::new("forbid_keyless_endpoints", FieldType::Boolean)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection".into(),
			column: "model_call_zdr".into(),
			old_definition: Some(
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean).with_not_null(true),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection".into(),
			column: "plan_connection_exempt".into(),
			old_definition: Some(
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean)
					.with_not_null(true),
			),
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection_history".into(),
			column: ColumnDefinition::new("forbid_plan_connections", FieldType::Boolean)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection_history".into(),
			column: ColumnDefinition::new("forbid_keyless_endpoints", FieldType::Boolean)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection_history".into(),
			column: "model_call_zdr".into(),
			old_definition: Some(
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean).with_not_null(true),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection_history".into(),
			column: "plan_connection_exempt".into(),
			old_definition: Some(
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean)
					.with_not_null(true),
			),
		})
}
