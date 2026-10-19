// reinhardt-migration-source: 1
// Reinhardt makemigrations model state for the Decision Attempt Route and outage
// retry. Physical PostgreSQL DDL and checks are applied by 0044_decision_routes.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0045_decision_routes_state", "execution")
		.add_dependency("execution", "0044_decision_routes")
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".to_string(),
			column: ColumnDefinition::new("retry", FieldType::Integer)
				.with_not_null(true)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".to_string(),
			column: ColumnDefinition::new("route", FieldType::Integer)
				.with_not_null(true)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
