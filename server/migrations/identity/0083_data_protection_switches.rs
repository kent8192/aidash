// reinhardt-migration-source: 1
// The Data Protection Level governs only OpenRouter calls (#962, #963, ADR 0137):
// Standard becomes No Restriction, Safety Retention is added, model-call ZDR
// goes, and Plan Connections and Keyless endpoints get Tenant switches.
// Rewriting stored policies is data, not schema: its assets are Reinhardt
// Query output from `identity::services::data_protection_backfill`, checked by
// `manage migrationseeds`, and open the Deployment Scope the forced row-level
// policies require. A Tenant without a stored policy gets its former default
// as a row first, so it keeps its level (#963 decision 5).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0083_data_protection_switches", "identity")
		.add_dependency("identity", "0082_remove_package_index_approvals")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "tenant_data_protection".into(),
			constraint: Constraint::Check {
				name: "tenant_data_protection_valid".into(),
				expression: "level IN ('standard', 'no_training', 'zero_data_retention') AND (level <> 'zero_data_retention' OR model_call_zdr)".into(),
			},
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection".into(),
			column: ColumnDefinition::new("forbid_plan_connections", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection".into(),
			column: ColumnDefinition::new("forbid_keyless_endpoints", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "tenant_data_protection_history".into(),
			constraint: Constraint::Check {
				name: "tenant_data_protection_history_valid".into(),
				expression: "level IN ('standard', 'no_training', 'zero_data_retention') AND (level <> 'zero_data_retention' OR model_call_zdr)".into(),
			},
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection_history".into(),
			column: ColumnDefinition::new("forbid_plan_connections", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "tenant_data_protection_history".into(),
			column: ColumnDefinition::new("forbid_keyless_endpoints", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0083_data_protection_switches.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0083_data_protection_switches.sql").into()),
		})
		// Restoring the column on rollback needs a value for existing rows;
		// it used to start true for every Tenant.
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection".into(),
			column: "model_call_zdr".into(),
			old_definition: Some(
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
			),
		})
		// Restoring the column on rollback needs a value for existing rows;
		// it used to start true for every Tenant.
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection".into(),
			column: "plan_connection_exempt".into(),
			old_definition: Some(
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
			),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tenant_data_protection".into(),
			constraint: Constraint::Check {
				name: "tenant_data_protection_valid".into(),
				expression: "level IN ('no_restriction', 'no_training', 'safety_retention', 'zero_data_retention')".into(),
			},
		})
		// Restoring the column on rollback needs a value for existing rows;
		// it used to start true for every Tenant.
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection_history".into(),
			column: "model_call_zdr".into(),
			old_definition: Some(
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
			),
		})
		// Restoring the column on rollback needs a value for existing rows;
		// it used to start true for every Tenant.
		.add_operation(Operation::DropColumn {
			table: "tenant_data_protection_history".into(),
			column: "plan_connection_exempt".into(),
			old_definition: Some(
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
			),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tenant_data_protection_history".into(),
			constraint: Constraint::Check {
				name: "tenant_data_protection_history_valid".into(),
				expression: "level IN ('no_restriction', 'no_training', 'safety_retention', 'zero_data_retention')".into(),
			},
		})
}
