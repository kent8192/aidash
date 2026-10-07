// reinhardt-migration-source: 1
//! Keep local Human references while Home owns remote continuation identities.
//! DDL exception: generated JSON CASE expressions require raw schema expressions.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0008_home_waiting_references", "execution")
 .database_only(true)
 .add_dependency("execution", "0007_model_state")
 .add_dependency("identity", "0011_home_human_requests_state")
 .add_operation(Operation::RunSQL { sql: include_str!("sql/forward/baseline_search_path.sql").into(), reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").into()) })
 .add_operation(Operation::DropConstraintDefinition { table: "runs".into(), constraint: Constraint::ForeignKey { name: "runs_human_request_ref".into(), columns: vec!["pending_human_request_id".into(), "id".into()], referenced_table: "human_requests".into(), referenced_columns: vec!["id".into(), "run_id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None } })
 .add_operation(Operation::DropColumn { table: "runs".into(), column: "pending_human_request_id".into(), old_definition: Some(ColumnDefinition::new("pending_human_request_id", FieldType::Uuid)
.with_generated(Some(GeneratedColumnDefinition::raw_sql(r#"CASE
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'external_approval'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
    ELSE NULL::uuid
END"#, GeneratedStorage::Stored)))) })
 .add_operation(Operation::AddColumn { table: "runs".into(), column: ColumnDefinition::new("pending_human_request_id", FieldType::Uuid)
.with_generated(Some(GeneratedColumnDefinition::raw_sql(r#"CASE
    WHEN ((context -> 'binding_snapshot'::text) -> 'remote'::text) = 'true'::jsonb THEN NULL::uuid
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'external_approval'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
    ELSE NULL::uuid
END"#, GeneratedStorage::Stored))), mysql_options: None })
 .add_operation(Operation::AddConstraintDefinition { table: "runs".into(), constraint: Constraint::ForeignKey { name: "runs_human_request_ref".into(), columns: vec!["pending_human_request_id".into(), "id".into()], referenced_table: "human_requests".into(), referenced_columns: vec!["id".into(), "run_id".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None } })
}
