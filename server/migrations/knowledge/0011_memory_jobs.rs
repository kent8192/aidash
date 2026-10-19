// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0011_memory_jobs", "knowledge")
 .add_dependency("knowledge", "0010_native_search_contract")
 .add_dependency("registry", "0012_memory_role_references")
.add_operation(Operation::CreateTable { name: "memory_model_operations".into(), columns: vec![
ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
ColumnDefinition::new("bank_id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("provider_id", FieldType::Text).with_not_null(true),
ColumnDefinition::new("provider_version", FieldType::Text).with_not_null(true),
ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
ColumnDefinition::new("calls", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("tokens", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("cost_micros", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
], constraints: vec![
	Constraint::ForeignKey {
		name: "memory_model_operations_tenant_root_fkey".to_owned(),
		columns: vec!["tenant".to_owned()],
		referenced_table: "tenants".to_owned(),
		referenced_columns: vec!["name".to_owned()],
		on_delete: ForeignKeyAction::NoAction,
		on_update: ForeignKeyAction::NoAction,
		deferrable: None,
	},
	Constraint::Unique {
		name: "memory_model_operations_tenant_id_key".to_owned(),
		columns: vec!["tenant".to_owned(), "id".to_owned()],
	},Constraint::PrimaryKey { name: "memory_model_operations_pkey".into(), columns: vec!["id".into()] }], without_rowid: None, interleave_in_parent: None, partition: None })
.add_operation(Operation::CreateTable { name: "memory_model_attempts".into(), columns: vec![
ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
ColumnDefinition::new("operation_id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("ordinal", FieldType::Integer).with_not_null(true),
ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
ColumnDefinition::new("tokens", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("cost_micros", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("output", FieldType::Jsonb).with_not_null(false),
ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
], constraints: vec![
	Constraint::ForeignKey {
		name: "memory_model_attempts_tenant_root_fkey".to_owned(),
		columns: vec!["tenant".to_owned()],
		referenced_table: "tenants".to_owned(),
		referenced_columns: vec!["name".to_owned()],
		on_delete: ForeignKeyAction::NoAction,
		on_update: ForeignKeyAction::NoAction,
		deferrable: None,
	},Constraint::PrimaryKey { name: "memory_model_attempts_pkey".into(), columns: vec!["id".into()] }], without_rowid: None, interleave_in_parent: None, partition: None })
.add_operation(Operation::AddConstraintDefinition { table: "memory_model_operations".into(), constraint: Constraint::ForeignKey { name: "memory_operation_bank".into(), columns: vec!["tenant".into(), "bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None } })

.add_operation(Operation::AddConstraintDefinition { table: "memory_model_attempts".into(), constraint: Constraint::ForeignKey { name: "memory_attempt_operation".into(), columns: vec!["tenant".into(), "operation_id".into()], referenced_table: "memory_model_operations".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None } })
.add_operation(Operation::AddConstraintDefinition { table: "memory_participants".into(), constraint: Constraint::ForeignKey { name: "memory_participant_agent".into(), columns: vec!["tenant".into(), "agent_id".into(), "agent_version".into()], referenced_table: "registry".into(), referenced_columns: vec!["tenant".into(), "id".into(), "version".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None } })

.add_operation(Operation::CreateTable {
name: "memory_publications".into(), columns: vec![
ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
ColumnDefinition::new("bank_id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("source_id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("source_revision", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("authority", FieldType::Jsonb).with_not_null(true),
ColumnDefinition::new("deleted", FieldType::Boolean).with_not_null(true),
ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
], constraints: vec![
	Constraint::ForeignKey {
		name: "memory_publications_tenant_root_fkey".to_owned(),
		columns: vec!["tenant".to_owned()],
		referenced_table: "tenants".to_owned(),
		referenced_columns: vec!["name".to_owned()],
		on_delete: ForeignKeyAction::NoAction,
		on_update: ForeignKeyAction::NoAction,
		deferrable: None,
	},Constraint::ForeignKey { name: "memory_publication_bank".into(), columns: vec!["tenant".into(), "bank_id".into()], referenced_table: "memory_banks".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
Constraint::ForeignKey { name: "memory_publication_source".into(), columns: vec!["tenant".into(), "source_id".into()], referenced_table: "memory_units".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None }], without_rowid: None, interleave_in_parent: None, partition: None })
.add_operation(Operation::CreateTable { name: "memory_task_participants".into(), columns: vec![
ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
ColumnDefinition::new("participant_id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("participant_revision", FieldType::BigInteger).with_not_null(true),
], constraints: vec![
	Constraint::ForeignKey {
		name: "memory_task_participants_tenant_root_fkey".to_owned(),
		columns: vec!["tenant".to_owned()],
		referenced_table: "tenants".to_owned(),
		referenced_columns: vec!["name".to_owned()],
		on_delete: ForeignKeyAction::NoAction,
		on_update: ForeignKeyAction::NoAction,
		deferrable: None,
	},Constraint::ForeignKey { name: "memory_assignment_task".into(), columns: vec!["tenant".into(), "task_id".into()], referenced_table: "tasks".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
Constraint::ForeignKey { name: "memory_assignment_participant".into(), columns: vec!["tenant".into(), "participant_id".into()], referenced_table: "memory_participants".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None }], without_rowid: None, interleave_in_parent: None, partition: None })
// Trigger DDL is not expressible by the typed migration API.
.add_operation(Operation::RunSQL { sql: include_str!("sql/forward/0011_memory_jobs.sql").into(), reverse_sql: Some(include_str!("sql/backward/0011_memory_jobs.sql").into()) })
.atomic(true)
}
