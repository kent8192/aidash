// reinhardt-migration-source: 1
// A Workbench test's default input limit grows from 20,000 to 40,000 bytes:
// the default tools a test declares (the Duty tools of #1011 and the longer
// `workspace_wait`) no longer fit in 20,000 bytes. Stored Tenant limits keep
// their values.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0079_workbench_input_bytes", "registry")
		.add_dependency("registry", "0078_duty_tools")
		.atomic(true)
		.add_operation(Operation::AlterColumn {
			table: "agent_test_limits".to_string(),
			column: "max_input_bytes".to_string(),
			old_definition: Some(
				ColumnDefinition::new("max_input_bytes", FieldType::Integer)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(Some("20000".to_string()))
					.with_generated(None)
					.with_domain_option(None),
			),
			new_definition: ColumnDefinition::new("max_input_bytes", FieldType::Integer)
				.with_not_null(true)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(Some("40000".to_string()))
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
}
