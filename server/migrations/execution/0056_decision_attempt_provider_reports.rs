// reinhardt-migration-source: 1
//! Persist actual provider facts with each charged attempt. Existing NULL rows
//! remain explicitly unknown; this migration invents no historical usage/cost.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0056_decision_attempt_provider_reports", "execution")
		.add_dependency("execution", "0055_approval_observation_lifecycle")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".into(),
			column: ColumnDefinition::new("provider_report", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
}
