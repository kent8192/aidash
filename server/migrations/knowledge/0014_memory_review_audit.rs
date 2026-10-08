// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0014_memory_review_audit", "knowledge")
        .add_dependency("knowledge", "0013_memory_reads")
        .add_operation(Operation::CreateTable {
            name: "memory_candidate_reviews".into(),
            columns: vec![
                ColumnDefinition::new("operation_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("candidate_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("bank_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("observed_revision", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("outcome", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![
                Constraint::ForeignKey { name: "memory_review_bank".into(), columns: vec!["bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "memory_review_revision".into(), expression: "observed_revision > 0".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        // Statement trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL {
            sql: "CREATE TRIGGER memory_candidate_reviews_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_candidate_reviews FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
            reverse_sql: Some("DROP TRIGGER memory_candidate_reviews_atomic ON memory_candidate_reviews;".into()),
        }).atomic(true)
}
