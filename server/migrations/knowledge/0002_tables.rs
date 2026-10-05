// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "knowledge")
		.database_only(true)
		.add_dependency("identity", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0002_sequences.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0002_sequences.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "semantic_agent_memory".to_owned(),
			columns: vec![
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("home_node", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "semantic_collections".to_owned(),
			columns: vec![
				ColumnDefinition::new("collection", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("vector", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("retired", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("last_error", FieldType::Text),
				ColumnDefinition::new("cleaned_at", FieldType::TimestampTz),
				ColumnDefinition::new("next_attempt", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "semantic_entries".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("workspace_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("key", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("source", FieldType::Jsonb)
				                .with_not_null(true), ColumnDefinition::new("agent", FieldType::Text),
				                ColumnDefinition::new("metadata", FieldType::Jsonb).with_not_null(true),
				                ColumnDefinition::new("revision", FieldType::BigInteger)
				                .with_not_null(true), ColumnDefinition::new("point_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("index_revision",
				                FieldType::BigInteger).with_not_null(true),
				                ColumnDefinition::new("deleted", FieldType::Boolean).with_not_null(true)
				                .with_default(Some("false".to_owned())), ColumnDefinition::new("state",
				                FieldType::Text).with_not_null(true), ColumnDefinition::new("attempts",
				                FieldType::Integer).with_not_null(true).with_default(Some("0"
				                .to_owned())), ColumnDefinition::new("last_error", FieldType::Text),
				                ColumnDefinition::new("created_by", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("authority", FieldType::Jsonb).with_not_null(true),
				                ColumnDefinition::new("updated_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned())),
				                ColumnDefinition::new("next_attempt", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "semantic_entries_authority".to_owned(),
				                expression :
				                r#"COALESCE(((jsonb_typeof(authority) = 'object'::text) AND (authority ? 'credential'::text) AND (jsonb_typeof((authority -> 'credential'::text)) = ANY (ARRAY['string'::text, 'null'::text])) AND (authority ? 'tenant'::text) AND (jsonb_typeof((authority -> 'tenant'::text)) = 'string'::text) AND (authority ? 'subject'::text) AND (jsonb_typeof((authority -> 'subject'::text)) = 'string'::text) AND (authority ? 'subjects'::text) AND (jsonb_typeof((authority -> 'subjects'::text)) = 'array'::text) AND (NOT jsonb_path_exists(authority, 'strict $."subjects"[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (((authority -> 'credential'::text) = 'null'::jsonb) OR ((authority ->> 'credential'::text) ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'::text))), false)"#
				                .to_owned() }, Constraint::Check { name : "semantic_entries_counters"
				                .to_owned(), expression :
				                r#"COALESCE(((revision > 0) AND (index_revision > 0) AND (attempts >= 0) AND
						CASE (source ->> 'kind'::text)
						WHEN 'memory'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'text'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'text'::text)) = 'string'::text) AND (deleted OR (length(btrim((source ->> 'text'::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0)))
						WHEN 'artifact'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
						WHEN 'message'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
						ELSE false
						END), false)"#
				                .to_owned() }, Constraint::Check { name : "semantic_entries_state_check"
				                .to_owned(), expression :
				                r#"(state = ANY (ARRAY['PENDING'::text, 'READY'::text, 'ERROR'::text, 'REVOKED'::text, 'DELETED'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "semantic_history".to_owned(),
			columns: vec![
				ColumnDefinition::new("sequence", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some(
						r#"nextval('public.semantic_history_sequence_seq'::regclass)"#.to_owned(),
					)),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("detail", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "semantic_indexes".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("workspace_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("tenant", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("revision",
				                FieldType::BigInteger).with_not_null(true), ColumnDefinition::new("spec",
				                FieldType::Jsonb).with_not_null(true),
				                ColumnDefinition::new("collection", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("updated_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "semantic_indexes_revision".to_owned(),
				                expression :
				                r#"COALESCE(((revision > 0) AND (revision < '9223372036854775807'::bigint) AND (jsonb_typeof(spec) = 'object'::text) AND
						CASE
						WHEN (jsonb_typeof(spec) = 'object'::text) THEN ((spec - ARRAY['embedding'::text, 'vector'::text, 'enabled'::text, 'auto_context'::text, 'max_sources'::text, 'max_results'::text, 'max_result_tokens'::text, 'max_input_bytes'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((spec -> 'enabled'::text)) = 'boolean'::text) AND (jsonb_typeof((spec -> 'auto_context'::text)) = 'boolean'::text) AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_sources'::text)) = 'number'::text) AND (((spec -> 'max_sources'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_sources'::text))::text)::numeric >= (1)::numeric) AND ((((spec -> 'max_sources'::text))::text)::numeric <= (1024)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_results'::text)) = 'number'::text) AND (((spec -> 'max_results'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_results'::text))::text)::numeric >= (1)::numeric) AND ((((spec -> 'max_results'::text))::text)::numeric <= (20)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_result_tokens'::text)) = 'number'::text) AND (((spec -> 'max_result_tokens'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_result_tokens'::text))::text)::numeric >= (128)::numeric) AND ((((spec -> 'max_result_tokens'::text))::text)::numeric <= (32768)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_input_bytes'::text)) = 'number'::text) AND (((spec -> 'max_input_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_input_bytes'::text))::text)::numeric >= (128)::numeric) AND ((((spec -> 'max_input_bytes'::text))::text)::numeric <= (32768)::numeric))
						ELSE false
						END AND
						CASE
						WHEN (jsonb_typeof((spec -> 'embedding'::text)) = 'object'::text) THEN (((spec -> 'embedding'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text, 'model'::text, 'model_version'::text, 'dimensions'::text]) = '{}'::jsonb)
						ELSE false
						END AND
						CASE
						WHEN (jsonb_typeof((spec -> 'vector'::text)) = 'object'::text) THEN (((spec -> 'vector'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((spec #>> '{embedding,provider}'::text[]) = 'openai'::text) AND public.aidash_valid_http_endpoint((spec #> '{embedding,endpoint}'::text[])) AND (((spec #> '{embedding,credential_env}'::text[]) IS NULL) OR ((spec #> '{embedding,credential_env}'::text[]) = 'null'::jsonb) OR ((jsonb_typeof((spec #> '{embedding,credential_env}'::text[])) = 'string'::text) AND ((spec #>> '{embedding,credential_env}'::text[]) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text))) AND ((spec #>> '{vector,provider}'::text[]) = 'qdrant'::text) AND public.aidash_valid_http_endpoint((spec #> '{vector,endpoint}'::text[])) AND (((spec #> '{vector,credential_env}'::text[]) IS NULL) OR ((spec #> '{vector,credential_env}'::text[]) = 'null'::jsonb) OR ((jsonb_typeof((spec #> '{vector,credential_env}'::text[])) = 'string'::text) AND ((spec #>> '{vector,credential_env}'::text[]) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text))) AND ((jsonb_typeof((spec #> '{embedding,model}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model}'::text[]), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length((spec #>> '{embedding,model}'::text[])) <= 256)) AND ((jsonb_typeof((spec #> '{embedding,model_version}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model_version}'::text[]), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length((spec #>> '{embedding,model_version}'::text[])) <= 128)) AND
						CASE
						WHEN ((jsonb_typeof((spec #> '{embedding,dimensions}'::text[])) = 'number'::text) AND (((spec #> '{embedding,dimensions}'::text[]))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric >= (1)::numeric) AND ((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric <= (8192)::numeric))
						ELSE false
						END), false)"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "semantic_points".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("content_digest", FieldType::Text),
				ColumnDefinition::new("collection", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("retired", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("last_error", FieldType::Text),
				ColumnDefinition::new("cleaned_at", FieldType::TimestampTz),
				ColumnDefinition::new("next_attempt", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "semantic_remote_attempts".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("operation_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("fence",
				                FieldType::BigInteger).with_not_null(true),
				                ColumnDefinition::new("cycle", FieldType::Integer).with_not_null(true),
				                ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("reservations", FieldType::Jsonb)
				                .with_not_null(true).with_default(Some(r#"'[]'::jsonb"#.to_owned())),
				                ColumnDefinition::new("error", FieldType::Text),
				                ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned())),
				                ColumnDefinition::new("dispatched_at", FieldType::TimestampTz),
				                ColumnDefinition::new("completed_at", FieldType::TimestampTz)
				            ],
				            constraints: vec![
				                Constraint::Check { name : "semantic_remote_attempts_state_check"
				                .to_owned(), expression :
				                r#"(state = ANY (ARRAY['RESERVING'::text, 'DISPATCHED'::text, 'COMPLETED'::text, 'ABORTED'::text, 'UNCERTAIN'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
				            name: "semantic_remote_operations".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("home_node", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("admission_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("digest", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("binding", FieldType::Jsonb)
				                .with_not_null(true), ColumnDefinition::new("state", FieldType::Text)
				                .with_not_null(true).with_default(Some(r#"'PENDING'::text"#.to_owned())),
				                ColumnDefinition::new("cycle", FieldType::Integer).with_not_null(true)
				                .with_default(Some("0".to_owned())), ColumnDefinition::new("failures",
				                FieldType::Integer).with_not_null(true).with_default(Some("0"
				                .to_owned())), ColumnDefinition::new("attempt_id", FieldType::Uuid),
				                ColumnDefinition::new("fence", FieldType::BigInteger).with_not_null(true)
				                .with_default(Some("0".to_owned())), ColumnDefinition::new("lease_until",
				                FieldType::TimestampTz), ColumnDefinition::new("next_attempt",
				                FieldType::TimestampTz), ColumnDefinition::new("error", FieldType::Text),
				                ColumnDefinition::new("receipt", FieldType::Jsonb),
				                ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "semantic_remote_operations_state_check"
				                .to_owned(), expression :
				                r#"(state = ANY (ARRAY['PENDING'::text, 'ACTIVE'::text, 'WAITING'::text, 'READY'::text, 'PAUSED'::text, 'INVALIDATED'::text, 'CANCELLED'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "semantic_remote_reads".to_owned(),
			columns: vec![
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("admission_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("content_digest", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "semantic_remote_receipts".to_owned(),
			columns: vec![
				ColumnDefinition::new("operation_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("receipt", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "semantic_run_reads".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "semantic_run_reads_revision".to_owned(),
				expression: r#"COALESCE((revision > 0), false)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0002_configuration.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0002_configuration.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
