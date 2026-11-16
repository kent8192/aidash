// reinhardt-migration-source: 1
// The Data Protection Level governs only OpenRouter calls (#962, #963, ADR 0137):
// Standard becomes No Restriction, Safety Retention is added, model-call ZDR
// goes, and Plan Connections and Keyless endpoints get Tenant switches.
// Rewriting stored levels is data, not schema, so it is the only SQL here;
// SET LOCAL opens the Deployment Scope the forced row-level policies require.
use reinhardt::db::migrations::prelude::*;

const LEVELS: &str =
	"level IN ('no_restriction', 'no_training', 'safety_retention', 'zero_data_retention')";

fn switches(table: &str) -> [Operation; 2] {
	["forbid_plan_connections", "forbid_keyless_endpoints"].map(|column| Operation::AddColumn {
		table: table.into(),
		column: ColumnDefinition::new(column, FieldType::Boolean)
			.with_not_null(true)
			.with_default(Some("false".to_owned())),
		mysql_options: None,
	})
}

fn removed(table: &str) -> [Operation; 2] {
	["model_call_zdr", "plan_connection_exempt"].map(|column| Operation::DropColumn {
		table: table.into(),
		column: column.into(),
		// Restoring the columns on rollback needs a value for existing rows;
		// both used to start true for every Tenant.
		old_definition: Some(
			ColumnDefinition::new(column, FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("true".to_owned())),
		),
	})
}

pub(super) fn migration() -> Migration {
	let mut migration = Migration::new("0080_data_protection_switches", "identity")
		.add_dependency("identity", "0079_bootstrap_reservations_state")
		.database_only(true)
		.atomic(true);
	for (table, check) in [
		("tenant_data_protection", "tenant_data_protection_valid"),
		(
			"tenant_data_protection_history",
			"tenant_data_protection_history_valid",
		),
	] {
		migration = migration.add_operation(Operation::DropConstraintDefinition {
			table: table.into(),
			constraint: Constraint::Check {
				name: check.into(),
				expression: "level IN ('standard', 'no_training', 'zero_data_retention') AND (level <> 'zero_data_retention' OR model_call_zdr)".into(),
			},
		});
		for operation in switches(table) {
			migration = migration.add_operation(operation);
		}
	}
	migration = migration.add_operation(Operation::RunSQL {
		sql: include_str!("sql/forward/0080_data_protection_switches.sql").into(),
		reverse_sql: Some(include_str!("sql/backward/0080_data_protection_switches.sql").into()),
	});
	for (table, check) in [
		("tenant_data_protection", "tenant_data_protection_valid"),
		(
			"tenant_data_protection_history",
			"tenant_data_protection_history_valid",
		),
	] {
		for operation in removed(table) {
			migration = migration.add_operation(operation);
		}
		migration = migration.add_operation(Operation::AddConstraintDefinition {
			table: table.into(),
			constraint: Constraint::Check {
				name: check.into(),
				expression: LEVELS.into(),
			},
		});
	}
	migration
}
