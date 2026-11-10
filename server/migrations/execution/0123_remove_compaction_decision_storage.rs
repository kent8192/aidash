// reinhardt-migration-source: 1
// Only the removed compaction Decider retained state and appended outcomes.
// Trigger and row-level-security DDL has no typed Reinhardt operation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0123_remove_compaction_decision_storage", "execution")
		.add_dependency("execution", "0122_invocation_refused_state")
		.database_only(true).atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0123_remove_compaction_decision_storage.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0123_remove_compaction_decision_storage.sql").into()),
		})
		// The historical model state omits these database defaults; remove them
		// explicitly so reversing the drop restores the original table schema.
		.add_operation(Operation::AlterColumn {
			table: "decision_states".into(), column: "created_at".into(),
			old_definition: Some(ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into()))),
			new_definition: ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true), mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "decision_outcomes".into(), column: "created_at".into(),
			old_definition: Some(ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into()))),
			new_definition: ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true), mysql_options: None,
		})
		.add_operation(Operation::DropNamedIndex {
			table: "decision_outcomes".into(), name: "decision_outcome_decision".into(), columns: vec!["decision_id".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None, concurrently: false,
			expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::DropNamedIndex {
			table: "decision_states".into(), name: "decision_state_expiry".into(), columns: vec!["expires_at".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None, concurrently: false,
			expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_states".into(),
			constraint: Constraint::ForeignKey {
					name: "decision_states_tenant_root_fkey".to_owned(),
					columns: vec!["tenant".to_owned()],
					referenced_table: "tenants".to_owned(),
					referenced_columns: vec!["name".to_owned()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_states".into(),
			constraint: Constraint::ForeignKey {
					name: "decision_state_evidence".to_owned(),
					columns: vec!["tenant".to_owned(), "run_id".to_owned(), "evidence_id".to_owned()],
					referenced_table: "decision_evidence".to_owned(),
					referenced_columns: vec!["tenant".to_owned(), "run_id".to_owned(), "id".to_owned()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_states".into(),
			constraint: Constraint::Unique {
					name: "decision_state_evidence_once".to_owned(),
					columns: vec!["evidence_id".to_owned()],
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_states".into(),
			constraint: Constraint::Check {
					name: "decision_state_digest".to_owned(),
					expression: "digest LIKE 'sha256:%'".to_owned(),
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_outcomes".into(),
			constraint: Constraint::ForeignKey {
					name: "decision_outcomes_tenant_root_fkey".to_owned(),
					columns: vec!["tenant".to_owned()],
					referenced_table: "tenants".to_owned(),
					referenced_columns: vec!["name".to_owned()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_outcomes".into(),
			constraint: Constraint::ForeignKey {
				name: "decision_outcome_run".into(),
				columns: vec!["tenant".into(), "run_id".into()],
				referenced_table: "runs".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_outcomes".into(),
			constraint: Constraint::Check {
					name: "decision_outcome_bounds".to_owned(),
					expression: "length(observed_status) BETWEEN 1 AND 256 AND (next_inference IS NULL OR length(next_inference) BETWEEN 1 AND 256)".to_owned(),
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "decision_evidence".into(),
			constraint: Constraint::Check {
				name: "decision_evidence_outcome".into(),
				expression: "outcome IN ('applied', 'staged', 'shadow', 'rejected', 'allowed', 'blocked') AND reason IN ('fits', 'insufficient', 'forbidden', 'provider_failure', 'authority_failure', 'journal_failure', 'invalid_answers', 'incomplete_answers', 'no_candidates', 'clear', 'data_exfiltration', 'destructive', 'beyond_request', 'injected', 'boundary', 'unavailable') AND jsonb_typeof(evidence) = 'object'".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "decision_evidence".into(),
			constraint: Constraint::Check {
				name: "decision_evidence_outcome".into(),
				expression: "outcome IN ('shadow', 'allowed', 'blocked') AND reason IN ('clear', 'data_exfiltration', 'destructive', 'beyond_request', 'injected', 'boundary', 'unavailable') AND jsonb_typeof(evidence) = 'object'".into(),
			},
		})
		.add_operation(Operation::DropNamedIndex {
			table: "decision_evidence".into(), name: "decision_evidence_final".into(), columns: vec!["decision_id".into()], unique: true,
			index_type: Some(IndexType::BTree), where_clause: Some("(outcome IN ('applied', 'staged', 'shadow', 'allowed', 'blocked'))".into()), concurrently: false,
			expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "decision_evidence".into(), name: "decision_evidence_final".into(), columns: vec!["decision_id".into()], unique: true,
			index_type: Some(IndexType::BTree), where_clause: Some("(outcome IN ('shadow', 'allowed', 'blocked'))".into()), concurrently: false,
			expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::DropTable { name: "decision_states".into() })
		.add_operation(Operation::DropTable { name: "decision_outcomes".into() })
}
