// reinhardt-migration-source: 1
// Research evidence retention and revocation (ADR 0045, ADR 0046). A Purged
// snapshot keeps its row, identity and digests, with the time and reason its
// contents were removed. An Evidence Revocation record has no reference to its
// Run, so it outlives the Run, and holds no source content.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0029_research_retention", "execution")
        .database_only(true)
        .add_dependency("execution", "0028_url_intents_model_state")
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("purged_at", FieldType::TimestampTz),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_snapshots".into(),
            column: ColumnDefinition::new("purge_reason", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "research_snapshots".into(),
            constraint: Constraint::Check {
                name: "research_snapshots_purge".into(),
                expression: "(purged_at IS NULL) = (purge_reason IS NULL) AND (purge_reason IS NULL OR (purge_reason IN ('retention', 'revocation') AND text_bytes = 0 AND lines = '[]'::jsonb))".into(),
            },
        })
        .add_operation(Operation::CreateIndex {
            table: "research_snapshots".into(),
            columns: vec!["id".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: Some("purged_at IS NULL".into()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::CreateTable {
            name: "research_revocations".into(),
            columns: vec![
                ColumnDefinition::new("run_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("tenant", FieldType::Text),
                ColumnDefinition::new("revoked_by", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("revoked_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("removed", FieldType::Jsonb).with_not_null(true),
            ], constraints: vec![
                Constraint::Check { name: "research_revocations_record".into(), expression: "char_length(reason) BETWEEN 1 AND 1000 AND jsonb_typeof(revoked_by) = 'object' AND jsonb_typeof(removed) = 'object'".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "research_revocations".into(),
            columns: vec!["tenant".into(), "revoked_at".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: Some("tenant IS NOT NULL".into()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .atomic(true)
}
