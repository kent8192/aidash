// reinhardt-migration-source: 1
// Provider results under the Research account terms (ADR 0058). An attempt
// records the account terms its result was retrieved under and when the
// result became a tombstone; Source records and Document snapshots name the
// attempt that created them; Evidence fragments keep a digest of their
// passage once its text is gone. A recorded termination outlives the
// account's declaration.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0034_research_account_terms", "execution")
        .database_only(true)
        .add_dependency("execution", "0033_research_event_run_index")
        .add_operation(Operation::AddColumn {
            table: "research_attempts".into(),
            column: ColumnDefinition::new("retention_account_id", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_attempts".into(),
            column: ColumnDefinition::new("retention_days", FieldType::Integer),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_attempts".into(),
            column: ColumnDefinition::new("purged_at", FieldType::TimestampTz),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_attempts".into(),
            column: ColumnDefinition::new("purge_reason", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "research_attempts".into(),
            constraint: Constraint::Check {
                name: "research_attempts_account_terms".into(),
                expression: "(retention_account_id IS NULL) = (retention_days IS NULL) AND (retention_days IS NULL OR retention_days > 0) AND (purged_at IS NULL) = (purge_reason IS NULL) AND (purge_reason IS NULL OR purge_reason = 'account_terms')".into(),
            },
        })
        // The sweeper examines only results still kept under account terms.
        .add_operation(Operation::CreateIndex {
            table: "research_attempts".into(),
            columns: vec!["id".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: Some("retention_account_id IS NOT NULL AND purged_at IS NULL".into()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_sources".into(),
            column: ColumnDefinition::new("attempt_key", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("attempt_key", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_fragments".into(),
            column: ColumnDefinition::new("passage_digest", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "research_snapshots".into(),
            constraint: Constraint::Check {
                name: "research_snapshots_purge".into(),
                expression: "(purged_at IS NULL) = (purge_reason IS NULL) AND (purge_reason IS NULL OR (purge_reason IN ('retention', 'revocation') AND text_bytes = 0 AND lines = '[]'::jsonb))".into(),
            },
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "research_snapshots".into(),
            constraint: Constraint::Check {
                name: "research_snapshots_purge".into(),
                expression: "(purged_at IS NULL) = (purge_reason IS NULL) AND (purge_reason IS NULL OR (purge_reason IN ('retention', 'revocation', 'account_terms') AND text_bytes = 0 AND lines = '[]'::jsonb))".into(),
            },
        })
        .add_operation(Operation::CreateIndex {
            table: "research_sources".into(),
            columns: vec!["run_id".into(), "attempt_key".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "research_snapshots".into(),
            columns: vec!["run_id".into(), "attempt_key".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::CreateTable {
            name: "research_account_terminations".into(),
            columns: vec![
                ColumnDefinition::new("account_id", FieldType::Text).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("terminated_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("deletion_after_termination_days", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("recorded_at", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![
                Constraint::Check { name: "research_account_terminations_period".into(), expression: "char_length(account_id) > 0 AND deletion_after_termination_days > 0".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .atomic(true)
}
