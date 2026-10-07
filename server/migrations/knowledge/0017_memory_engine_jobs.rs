// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0017_memory_engine_jobs", "knowledge")
 .add_dependency("knowledge", "0016_memory_bank_settings")
 .add_operation(Operation::AddColumn { table: "memory_units".into(), column: ColumnDefinition::new("mental_model", FieldType::Jsonb), mysql_options: None })
 .add_operation(Operation::AddColumn { table: "memory_history".into(), column: ColumnDefinition::new("mental_model", FieldType::Jsonb), mysql_options: None })
 .add_operation(Operation::AddColumn { table: "memory_candidates".into(), column: ColumnDefinition::new("mental_model", FieldType::Jsonb), mysql_options: None })
 .add_operation(Operation::CreateTable {
 name: "memory_engine_jobs".into(), columns: vec![
 ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
 ColumnDefinition::new("bank_id", FieldType::Uuid).with_not_null(true),
 ColumnDefinition::new("provider_id", FieldType::Text).with_not_null(true),
 ColumnDefinition::new("provider_version", FieldType::Text).with_not_null(true),
 ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
 ColumnDefinition::new("input", FieldType::Jsonb).with_not_null(true),
 ColumnDefinition::new("authority", FieldType::Jsonb).with_not_null(true),
 ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
 ColumnDefinition::new("attempts", FieldType::Integer).with_not_null(true),
 ColumnDefinition::new("claim", FieldType::Uuid),
 ColumnDefinition::new("next_attempt", FieldType::TimestampTz).with_not_null(true),
 ColumnDefinition::new("last_error", FieldType::Text),
 ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
 ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
 ], constraints: vec![
 Constraint::ForeignKey { name: "memory_job_bank".into(), columns: vec!["bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
 Constraint::ForeignKey { name: "memory_job_provider".into(), columns: vec!["provider_id".into(),"provider_version".into()], referenced_table: "registry".into(), referenced_columns: vec!["id".into(),"version".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
 Constraint::Check { name: "memory_job_state".into(), expression: "state IN ('pending','running','complete','blocked','failed') AND attempts >= 0 AND kind IN ('learn','observation','mental_model')".into() },
 ], without_rowid: None, interleave_in_parent: None, partition: None,
 })
 // Statement trigger DDL is unsupported by the typed migration API.
 .add_operation(Operation::RunSQL { sql: "CREATE TRIGGER memory_engine_jobs_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_engine_jobs FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(), reverse_sql: Some("DROP TRIGGER memory_engine_jobs_atomic ON memory_engine_jobs;".into()) })
 .atomic(true)
}
