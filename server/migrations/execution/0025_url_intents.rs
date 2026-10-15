// reinhardt-migration-source: 1
// URL intents: the pages a person asked one Run to read, recorded beside the
// Task (no input key) or run input that carried them, never in its text.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0025_url_intents", "execution")
        .add_dependency("execution", "0024_estimate_calibration_state")
        .add_operation(Operation::CreateTable {
            name: "url_intents".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("input_key", FieldType::Text),
                ColumnDefinition::new("pages", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("requested_by", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into())),
            ], constraints: vec![
                Constraint::ForeignKey { name: "url_intents_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                // At most five readable pages per input; the host validates each.
                Constraint::Check { name: "url_intents_pages".into(), expression: "jsonb_typeof(pages) = 'array' AND jsonb_array_length(pages) BETWEEN 1 AND 5".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "url_intents".into(),
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
