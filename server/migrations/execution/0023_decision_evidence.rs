// reinhardt-migration-source: 1
//! Decision Evidence, its charged Decision Attempts, optionally retained
//! classification state and appended outcomes. Every table cascades with its Run
//! and records the Decider by its exact pin, never by a Registry reference.
//! Physical DDL precedes the `0024_decision_evidence_model_state` snapshot.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0023_decision_evidence", "execution")
		.database_only(true)
		.add_dependency("execution", "0022_outbound_approval_activation")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "decision_attempts".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("decision_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("step", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("input_revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("input_digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("worker", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("decider", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("binding_restrictions", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("request_digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("state_digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("questions", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("mode", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("owner_receipts", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("answers", FieldType::Jsonb),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".to_owned())),
				ColumnDefinition::new("finished_at", FieldType::TimestampTz),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "decision_attempt_run".to_owned(),
					columns: vec!["run_id".to_owned()],
					referenced_table: "runs".to_owned(),
					referenced_columns: vec!["id".to_owned()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "decision_attempt_status".to_owned(),
					expression: "status IN ('pending', 'answered', 'failed', 'uncertain', 'not_dispatched') AND (status = 'pending') = (finished_at IS NULL) AND (status = 'answered') = (answers IS NOT NULL)".to_owned(),
				},
				Constraint::Check {
					name: "decision_attempt_shape".to_owned(),
					expression: "mode IN ('shadow', 'enforce') AND step >= 0 AND input_revision >= 0 AND run_revision >= 0 AND request_digest LIKE 'sha256:%' AND state_digest LIKE 'sha256:%' AND input_digest LIKE 'sha256:%' AND jsonb_typeof(questions) = 'array' AND jsonb_array_length(questions) > 0 AND jsonb_typeof(owner_receipts) = 'object' AND owner_receipts <> '{}'::jsonb AND (answers IS NULL OR jsonb_typeof(answers) = 'object')".to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_attempts".to_owned(),
			name: "decision_attempt_decision".to_owned(),
			columns: vec!["decision_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_attempts".to_owned(),
			name: "decision_attempt_run_lookup".to_owned(),
			columns: vec!["run_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateTable {
			name: "decision_evidence".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("decision_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("evidence", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".to_owned())),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "decision_evidence_run".to_owned(),
					columns: vec!["run_id".to_owned()],
					referenced_table: "runs".to_owned(),
					referenced_columns: vec!["id".to_owned()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				// Retained state can reference only its own Run's evidence.
				Constraint::Unique {
					name: "decision_evidence_run_identity".to_owned(),
					columns: vec!["run_id".to_owned(), "id".to_owned()],
				},
				Constraint::Check {
					name: "decision_evidence_outcome".to_owned(),
					expression: "outcome IN ('applied', 'shadow', 'rejected') AND reason IN ('fits', 'insufficient', 'forbidden', 'provider_failure', 'authority_failure', 'journal_failure', 'invalid_answers', 'no_candidates') AND jsonb_typeof(evidence) = 'object'".to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// An applied or shadow decision is final; only rejected snapshots repeat.
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_evidence".to_owned(),
			name: "decision_evidence_final".to_owned(),
			columns: vec!["decision_id".to_owned()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(outcome IN ('applied', 'shadow'))".to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_evidence".to_owned(),
			name: "decision_evidence_decision".to_owned(),
			columns: vec!["decision_id".to_owned(), "created_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateTable {
			name: "decision_states".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("evidence_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".to_owned())),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "decision_state_evidence".to_owned(),
					columns: vec!["run_id".to_owned(), "evidence_id".to_owned()],
					referenced_table: "decision_evidence".to_owned(),
					referenced_columns: vec!["run_id".to_owned(), "id".to_owned()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Unique {
					name: "decision_state_evidence_once".to_owned(),
					columns: vec!["evidence_id".to_owned()],
				},
				Constraint::Check {
					name: "decision_state_digest".to_owned(),
					expression: "digest LIKE 'sha256:%'".to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_states".to_owned(),
			name: "decision_state_expiry".to_owned(),
			columns: vec!["expires_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateTable {
			name: "decision_outcomes".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("decision_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("next_inference", FieldType::Text),
				ColumnDefinition::new("observed_status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".to_owned())),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "decision_outcome_run".to_owned(),
					columns: vec!["run_id".to_owned()],
					referenced_table: "runs".to_owned(),
					referenced_columns: vec!["id".to_owned()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "decision_outcome_bounds".to_owned(),
					expression: "length(observed_status) BETWEEN 1 AND 256 AND (next_inference IS NULL OR length(next_inference) BETWEEN 1 AND 256)".to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_outcomes".to_owned(),
			name: "decision_outcome_decision".to_owned(),
			columns: vec!["decision_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// Statement trigger DDL is unsupported by the typed migration API.
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER decision_attempts_atomic BEFORE INSERT OR UPDATE OR DELETE ON decision_attempts FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".to_owned(),
			reverse_sql: Some("DROP TRIGGER decision_attempts_atomic ON decision_attempts;".to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER decision_evidence_atomic BEFORE INSERT OR UPDATE OR DELETE ON decision_evidence FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".to_owned(),
			reverse_sql: Some("DROP TRIGGER decision_evidence_atomic ON decision_evidence;".to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER decision_states_atomic BEFORE INSERT OR UPDATE OR DELETE ON decision_states FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".to_owned(),
			reverse_sql: Some("DROP TRIGGER decision_states_atomic ON decision_states;".to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER decision_outcomes_atomic BEFORE INSERT OR UPDATE OR DELETE ON decision_outcomes FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".to_owned(),
			reverse_sql: Some("DROP TRIGGER decision_outcomes_atomic ON decision_outcomes;".to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
		.atomic(true)
}
