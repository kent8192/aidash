// reinhardt-migration-source: 1
//! Decision Evidence v3 (#108 Part B, ADR 0056). A charged attempt commits to
//! the definitions of the questions it asked and records the Run's resume cycle
//! its decision identity commits to, a decision whose pruning the
//! Summary Stage completed is `staged`, and incomplete durable answers have a
//! reason of their own. `staged` is final like `applied` and `shadow`.
//! Aidash is undeployed, so v2 rows are not migrated: the new attempt column
//! has no default. Physical DDL precedes the `0036_decision_evidence_v3_state`
//! snapshot.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0035_decision_evidence_v3", "execution")
		.database_only(true)
		.add_dependency("execution", "0034_tokenized_estimate_confidence")
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".into(),
			column: ColumnDefinition::new("question_digest", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "decision_attempts".into(),
			column: ColumnDefinition::new("cycle", FieldType::Integer).with_not_null(true),
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
				expression: "mode IN ('shadow', 'enforce') AND step >= 0 AND cycle >= 0 AND input_revision >= 0 AND run_revision >= 0 AND request_digest LIKE 'sha256:%' AND state_digest LIKE 'sha256:%' AND input_digest LIKE 'sha256:%' AND question_digest LIKE 'sha256:%' AND jsonb_typeof(questions) = 'array' AND jsonb_array_length(questions) > 0 AND jsonb_typeof(owner_receipts) = 'object' AND owner_receipts <> '{}'::jsonb AND (answers IS NULL OR jsonb_typeof(answers) = 'object')".into(),
			},
		})
		.add_operation(Operation::DropConstraint {
			table: "decision_evidence".into(),
			constraint_name: "decision_evidence_outcome".into(),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "decision_evidence".into(),
			constraint: Constraint::Check {
				name: "decision_evidence_outcome".into(),
				expression: "outcome IN ('applied', 'staged', 'shadow', 'rejected') AND reason IN ('fits', 'insufficient', 'forbidden', 'provider_failure', 'authority_failure', 'journal_failure', 'invalid_answers', 'incomplete_answers', 'no_candidates') AND jsonb_typeof(evidence) = 'object'".into(),
			},
		})
		.add_operation(Operation::DropNamedIndex {
			table: "decision_evidence".into(),
			name: "decision_evidence_final".into(),
			columns: vec!["decision_id".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(outcome IN ('applied', 'shadow'))".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// An applied, staged or shadow decision is final; only rejected snapshots repeat.
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_evidence".into(),
			name: "decision_evidence_final".into(),
			columns: vec!["decision_id".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(outcome IN ('applied', 'staged', 'shadow'))".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
