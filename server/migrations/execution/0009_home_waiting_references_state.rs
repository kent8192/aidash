// reinhardt-migration-source: 1
//! Native model state for local-only Human continuation references.
//! DDL exception: generated JSON CASE expressions require raw schema expressions.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0009_home_waiting_references_state", "execution")
 .state_only(true)
 .add_dependency("execution", "0008_home_waiting_references")
 .add_operation(Operation::AlterColumn { table: "runs".into(), column: "pending_human_request_id".into(), old_definition: Some(ColumnDefinition::new("pending_human_request_id", FieldType::Uuid)
.with_generated(Some(GeneratedColumnDefinition::raw_sql(r#"CASE
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'external_approval'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
    ELSE NULL::uuid
END"#, GeneratedStorage::Stored)))), new_definition: ColumnDefinition::new("pending_human_request_id", FieldType::Uuid)
.with_generated(Some(GeneratedColumnDefinition::raw_sql(r#"CASE
    WHEN ((context -> 'binding_snapshot'::text) -> 'remote'::text) = 'true'::jsonb THEN NULL::uuid
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'external_approval'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
    ELSE NULL::uuid
END"#, GeneratedStorage::Stored))), mysql_options: None })
}
