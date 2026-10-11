// reinhardt-migration-source: 1
// Run-scoped Research evidence: provider attempts attributed to the Search
// service operator, Source records, Document snapshots and Evidence fragments.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0014_research_evidence", "execution")
        .add_dependency("execution", "0013_inference_usage_model_state")
        .add_operation(Operation::CreateTable {
            name: "research_attempts".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("invocation_key", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("operation", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("provider", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("destination", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("account_id", FieldType::Text),
                ColumnDefinition::new("price_micro_usd", FieldType::BigInteger),
                ColumnDefinition::new("outcome", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("result", FieldType::Jsonb),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into())),
                ColumnDefinition::new("completed_at", FieldType::TimestampTz),
            ], constraints: vec![
                Constraint::ForeignKey { name: "research_attempts_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Unique { name: "research_attempts_invocation".into(), columns: vec!["run_id".into(), "invocation_key".into()] },
                Constraint::Check { name: "research_attempts_operation".into(), expression: "operation IN ('search', 'fetch')".into() },
                Constraint::Check { name: "research_attempts_outcome".into(), expression: "outcome IN ('dispatched', 'completed', 'failed', 'unknown') AND (outcome = 'dispatched') = (completed_at IS NULL) AND (outcome = 'dispatched') = (result IS NULL)".into() },
                Constraint::Check { name: "research_attempts_attribution".into(), expression: "(account_id IS NULL) = (price_micro_usd IS NULL) AND (price_micro_usd IS NULL OR price_micro_usd >= 0)".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "research_attempts".into(),
            columns: vec!["account_id".into(), "created_at".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: Some("account_id IS NOT NULL".into()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::CreateTable {
            name: "research_sources".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("url", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("title", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into())),
            ], constraints: vec![
                Constraint::ForeignKey { name: "research_sources_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Unique { name: "research_sources_run_identity".into(), columns: vec!["run_id".into(), "id".into()] },
                Constraint::Check { name: "research_sources_bounds".into(), expression: "length(url) BETWEEN 1 AND 4096 AND length(title) <= 512".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "research_snapshots".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("source_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("requested_url", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("url", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("title", FieldType::Text),
                ColumnDefinition::new("media_type", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("fetched_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("raw_digest", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("text_digest", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("completeness", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("lines", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("text_bytes", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into())),
            ], constraints: vec![
                // A snapshot's source must belong to the same Run.
                Constraint::ForeignKey { name: "research_snapshots_source".into(), columns: vec!["run_id".into(), "source_id".into()], referenced_table: "research_sources".into(), referenced_columns: vec!["run_id".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Unique { name: "research_snapshots_run_identity".into(), columns: vec!["run_id".into(), "id".into()] },
                Constraint::Check { name: "research_snapshots_completeness".into(), expression: "completeness IN ('complete', 'text_limit')".into() },
                Constraint::Check { name: "research_snapshots_digests".into(), expression: "raw_digest LIKE 'sha256:%' AND text_digest LIKE 'sha256:%' AND text_bytes >= 0 AND jsonb_typeof(lines) = 'array'".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "research_fragments".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("snapshot_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("text", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("line_start", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("line_end", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("pages", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into())),
            ], constraints: vec![
                // A fragment is evidence only for the Run whose snapshot it came from.
                Constraint::ForeignKey { name: "research_fragments_snapshot".into(), columns: vec!["run_id".into(), "snapshot_id".into()], referenced_table: "research_snapshots".into(), referenced_columns: vec!["run_id".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "research_fragments_lines".into(), expression: "line_start >= 1 AND line_end >= line_start AND jsonb_typeof(pages) = 'array'".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .atomic(true)
}
