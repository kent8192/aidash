// reinhardt-migration-source: 1
// PDF pages that carry no text (#312, ADR 0053). A snapshot records its page
// count, the pages that carry no text and the digest and size of its original.
// The original itself lives in research_originals, so that reading a snapshot
// never loads it; it is removed with the snapshot's contents by a purge or an
// Evidence Revocation, and with its Run.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0029_research_originals", "execution")
        .database_only(true)
        .add_dependency("execution", "0028_research_retention_state")
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("page_count", FieldType::Integer),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("pages_without_text", FieldType::Jsonb)
                .with_not_null(true)
                .with_default(Some("'[]'::jsonb".into())),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("original_digest", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("original_bytes", FieldType::BigInteger),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("original_retained", FieldType::Boolean)
                .with_not_null(true)
                .with_default(Some("false".into())),
            mysql_options: None,
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "research_snapshots".into(),
            constraint: Constraint::Check {
                name: "research_snapshots_pages".into(),
                expression: "(page_count IS NULL OR page_count >= 1) AND jsonb_typeof(pages_without_text) = 'array' AND (original_digest IS NULL) = (original_bytes IS NULL) AND (original_digest IS NULL OR original_digest LIKE 'sha256:%') AND (original_bytes IS NULL OR original_bytes BETWEEN 1 AND 16777216) AND (NOT original_retained OR (original_digest IS NOT NULL AND purged_at IS NULL))".into(),
            },
        })
        .add_operation(Operation::CreateTable {
            name: "research_originals".into(),
            columns: vec![
                ColumnDefinition::new("snapshot_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("bytes", FieldType::Bytea).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![
                Constraint::ForeignKey { name: "research_originals_snapshot".into(), columns: vec!["run_id".into(), "snapshot_id".into()], referenced_table: "research_snapshots".into(), referenced_columns: vec!["run_id".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "research_originals_bytes".into(), expression: "octet_length(bytes) BETWEEN 1 AND 16777216".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "research_originals".into(),
            columns: vec!["run_id".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .atomic(true)
}
