// reinhardt-migration-source: 1
// Page-image Evidence fragments (#312, ADR 0053). A fragment is a passage of
// snapshot lines or one page image, never both: a page image has no lines,
// names its page, media type, size and digest, and keeps its bytes in
// research_page_images so that reading fragments never loads them. An
// Evidence Revocation deletes the bytes; a retention purge keeps them, as it
// keeps a passage's text.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0037_research_page_images", "execution")
        .database_only(true)
        .add_dependency("execution", "0036_row_level_security")
        .add_operation(Operation::AddColumn {
            table: "research_fragments".into(),
            column: ColumnDefinition::new("image_page", FieldType::Integer),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_fragments".into(),
            column: ColumnDefinition::new("image_media_type", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_fragments".into(),
            column: ColumnDefinition::new("image_width", FieldType::Integer),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_fragments".into(),
            column: ColumnDefinition::new("image_height", FieldType::Integer),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "research_fragments".into(),
            column: ColumnDefinition::new("image_digest", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "research_fragments".into(),
            constraint: Constraint::Check {
                name: "research_fragments_lines".into(),
                expression: "line_start >= 1 AND line_end >= line_start AND jsonb_typeof(pages) = 'array'".into(),
            },
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "research_fragments".into(),
            constraint: Constraint::Check {
                name: "research_fragments_lines".into(),
                expression: "jsonb_typeof(pages) = 'array' AND ((image_page IS NULL AND image_media_type IS NULL AND image_width IS NULL AND image_height IS NULL AND image_digest IS NULL AND line_start >= 1 AND line_end >= line_start) OR (image_page >= 1 AND line_start = 0 AND line_end = 0 AND image_media_type = 'image/png' AND image_width BETWEEN 1 AND 1568 AND image_height BETWEEN 1 AND 1568 AND image_digest LIKE 'sha256:%'))".into(),
            },
        })
        .add_operation(Operation::CreateTable {
            name: "research_page_images".into(),
            columns: vec![
                ColumnDefinition::new("fragment_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("bytes", FieldType::Bytea).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![
                Constraint::ForeignKey { name: "research_page_images_fragment".into(), columns: vec!["fragment_id".into()], referenced_table: "research_fragments".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "research_page_images_bytes".into(), expression: "octet_length(bytes) BETWEEN 1 AND 1048576".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "research_page_images".into(),
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
