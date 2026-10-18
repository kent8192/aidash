// reinhardt-migration-source: 1
// Reinhardt makemigrations model state for the Decision Attempt question
// commitment and resume cycle. Physical PostgreSQL DDL and checks are applied by
// 0036_decision_evidence_v3.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0037_decision_evidence_v3_state", "execution")
		.add_dependency("execution", "0036_decision_evidence_v3")
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".to_string(),
			column: ColumnDefinition::new("cycle", FieldType::Integer)
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
			column: ColumnDefinition::new("question_digest", FieldType::Text)
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
