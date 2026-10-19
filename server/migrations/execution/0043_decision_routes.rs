// reinhardt-migration-source: 1
//! Decider Routes (#153, ADR 0066). A Decision Attempt records the Route it
//! asked and the step's outage retry, which its decision identity commits to,
//! so each Route and each retry is a decision of its own. Aidash is undeployed,
//! so earlier attempts are not migrated: the new columns have no default, and
//! the migration fails while a Decision Attempt exists. Physical DDL precedes
//! the `0044_decision_routes_state` snapshot.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0043_decision_routes", "execution")
		.database_only(true)
		.add_dependency("execution", "0042_decision_evidence_v3_state")
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".into(),
			column: ColumnDefinition::new("retry", FieldType::Integer).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".into(),
			column: ColumnDefinition::new("route", FieldType::Integer).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraint {
			table: "decision_attempts".into(),
			constraint_name: "decision_attempt_shape".into(),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "decision_attempts".into(),
			constraint: Constraint::Check {
				name: "decision_attempt_shape".into(),
				expression: "mode IN ('shadow', 'enforce') AND step >= 0 AND cycle >= 0 AND retry >= 0 AND route >= 0 AND input_revision >= 0 AND run_revision >= 0 AND request_digest LIKE 'sha256:%' AND state_digest LIKE 'sha256:%' AND input_digest LIKE 'sha256:%' AND question_digest LIKE 'sha256:%' AND jsonb_typeof(questions) = 'array' AND jsonb_array_length(questions) > 0 AND jsonb_typeof(owner_receipts) = 'object' AND owner_receipts <> '{}'::jsonb AND (answers IS NULL OR jsonb_typeof(answers) = 'object')".into(),
			},
		})
}
