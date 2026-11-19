// reinhardt-migration-source: 1
// A Workbench test's default limits grow from 20,000 to 40,000 input bytes
// and from 16,384 to 32,768 total tokens: the default tools a test declares
// (the Duty tools of #1011 and the longer `workspace_wait`) and an output
// reservation no longer fit. Stored Tenant limits keep their values.
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
		.add_operation(Operation::AlterColumn {
			table: "agent_test_limits".to_string(),
			column: "max_total_tokens".to_string(),
			old_definition: Some(
				ColumnDefinition::new("max_total_tokens", FieldType::Integer)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(Some("16384".to_string()))
					.with_generated(None)
					.with_domain_option(None),
			),
			new_definition: ColumnDefinition::new("max_total_tokens", FieldType::Integer)
				.with_not_null(true)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(Some("32768".to_string()))
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
}
