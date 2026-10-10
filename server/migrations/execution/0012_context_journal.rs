// reinhardt-migration-source: 1
//! Append-only Context Journal and pre-I/O Compaction Attempts.
//! `runs.context` remains the lossy projection; the journal keeps each original
//! event. Physical DDL precedes the `0013_context_journal_model_state` snapshot.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0012_context_journal", "execution")
		.database_only(true)
		.add_dependency("execution", "0011_binding_memory_merge")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "run_context_events".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("seq", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("origin", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("event", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".to_owned())),
			],
			constraints: vec![
				Constraint::PrimaryKey {
					name: "run_context_events_pkey".to_owned(),
					columns: vec!["run_id".to_owned(), "seq".to_owned()],
				},
				Constraint::ForeignKey {
					name: "run_context_event_run".to_owned(),
					columns: vec!["run_id".to_owned()],
					referenced_table: "runs".to_owned(),
					referenced_columns: vec!["id".to_owned()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "run_context_event_seq".to_owned(),
					expression: "seq > 0".to_owned(),
				},
				Constraint::Check {
					name: "run_context_event_origin".to_owned(),
					expression: "origin IN ('appended', 'imported')".to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "context_compaction_attempts".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("stage", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("policy_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("provider", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_from_seq", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("source_through_seq", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("base_revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("observed_input_seq", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("before_tokens", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text),
				ColumnDefinition::new("reason", FieldType::Text),
				ColumnDefinition::new("candidate_digest", FieldType::Text),
				ColumnDefinition::new("after_tokens", FieldType::BigInteger),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".to_owned())),
				ColumnDefinition::new("settled_at", FieldType::TimestampTz),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "context_compaction_attempt_run".to_owned(),
					columns: vec!["run_id".to_owned()],
					referenced_table: "runs".to_owned(),
					referenced_columns: vec!["id".to_owned()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "context_compaction_attempt_stage".to_owned(),
					expression: "stage IN ('prune', 'summary')".to_owned(),
				},
				Constraint::Check {
					name: "context_compaction_attempt_range".to_owned(),
					expression: "source_from_seq >= 0 AND source_through_seq >= source_from_seq AND before_tokens >= 0 AND (after_tokens IS NULL OR after_tokens >= 0)".to_owned(),
				},
				Constraint::Check {
					name: "context_compaction_attempt_outcome".to_owned(),
					expression: "outcome IS NULL OR outcome IN ('adopted', 'insufficient', 'unavailable', 'invalid', 'unauthorized', 'abandoned')".to_owned(),
				},
				Constraint::Check {
					name: "context_compaction_attempt_settled".to_owned(),
					expression: "(outcome IS NULL) = (settled_at IS NULL)".to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "context_compaction_attempts".to_owned(),
			name: "context_compaction_attempt_stage_lookup".to_owned(),
			columns: vec!["run_id".to_owned(), "stage".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "context_compaction_attempts".to_owned(),
			name: "context_compaction_attempt_open".to_owned(),
			columns: vec!["run_id".to_owned()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(outcome IS NULL)".to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// Statement trigger DDL is unsupported by the typed migration API.
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER run_context_events_atomic BEFORE INSERT OR UPDATE OR DELETE ON run_context_events FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".to_owned(),
			reverse_sql: Some("DROP TRIGGER run_context_events_atomic ON run_context_events;".to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: "CREATE TRIGGER context_compaction_attempts_atomic BEFORE INSERT OR UPDATE OR DELETE ON context_compaction_attempts FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".to_owned(),
			reverse_sql: Some("DROP TRIGGER context_compaction_attempts_atomic ON context_compaction_attempts;".to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
		.atomic(true)
}
