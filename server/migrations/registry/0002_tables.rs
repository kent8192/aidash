// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "registry")
		.database_only(true)
		.add_dependency("marketplace", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0002_sequences.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0002_sequences.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "agent_draft_registrations".to_owned(),
			columns: vec![
				ColumnDefinition::new("draft_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("release_notes", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"''::text"#.to_owned())),
				ColumnDefinition::new("source_id", FieldType::Text),
				ColumnDefinition::new("source_version", FieldType::Text),
				ColumnDefinition::new("behavioral_tested", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("registered_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_draft_shares".to_owned(),
			columns: vec![
				ColumnDefinition::new("draft_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("can_edit", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("documents_digest", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_drafts".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("owner", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("managed_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("entry", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("documents", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("release_notes", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"''::text"#.to_owned())),
				ColumnDefinition::new("source_id", FieldType::Text),
				ColumnDefinition::new("source_version", FieldType::Text),
				ColumnDefinition::new("archived", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_incident_events".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some(
						r#"nextval('public.agent_incident_events_id_seq'::regclass)"#.to_owned(),
					)),
				ColumnDefinition::new("incident_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("change", FieldType::Jsonb).with_not_null(true),
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
			name: "agent_incidents".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("severity", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("status", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"'open'::text"#.to_owned())),
				ColumnDefinition::new("archived", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("owner", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("notes", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("evidence", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
				ColumnDefinition::new("resolved_at", FieldType::TimestampTz),
				ColumnDefinition::new("evidence_expires_at", FieldType::TimestampTz),
				ColumnDefinition::new("evidence_expired_at", FieldType::TimestampTz),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_knowledge".to_owned(),
			columns: vec![
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("documents", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_test_limits".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("max_input_bytes", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("20000".to_owned())),
				ColumnDefinition::new("max_output_tokens", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("2048".to_owned())),
				ColumnDefinition::new("max_total_tokens", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("16384".to_owned())),
				ColumnDefinition::new("max_steps", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("12".to_owned())),
				ColumnDefinition::new("max_duration_secs", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("60".to_owned())),
				ColumnDefinition::new("max_concurrent", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("2".to_owned())),
				ColumnDefinition::new("payload_days", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("30".to_owned())),
				ColumnDefinition::new("incident_evidence_days", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("90".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_test_profiles".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("enabled", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
				ColumnDefinition::new("rules", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "agent_test_sessions".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("draft_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scenario", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("conversation", FieldType::Jsonb),
				ColumnDefinition::new("tool_calls", FieldType::Jsonb),
				ColumnDefinition::new("usage", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("error", FieldType::Text),
				ColumnDefinition::new("active_slot", FieldType::Uuid),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("expired_at", FieldType::TimestampTz),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "installations".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("config", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("installed_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "installations_config".to_owned(),
				expression: r#"COALESCE((jsonb_typeof(config) = 'object'::text), false)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "packages".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("version", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("manifest", FieldType::Jsonb).with_not_null(true),
				                ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("manifest_source", FieldType::Text)
				                .with_not_null(true).with_default(Some(r#"''::text"#.to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "packages_digest".to_owned(), expression :
				                r#"COALESCE((public.aidash_package_source_matches(manifest, manifest_source) AND (digest = ('sha256:'::text || encode(sha256(convert_to(manifest_source, 'UTF8'::name)), 'hex'::text)))), false)"#
				                .to_owned() }, Constraint::Check { name : "packages_identity".to_owned(),
				                expression :
				                r#"COALESCE(((jsonb_typeof(manifest) = 'object'::text) AND ((manifest #> '{entity,id}'::text[]) = to_jsonb(id)) AND ((manifest #> '{entity,version}'::text[]) = to_jsonb(version)) AND
						CASE
						WHEN (jsonb_typeof(manifest) = 'object'::text) THEN ((manifest - ARRAY['entity'::text, 'author'::text, 'permissions'::text, 'dependencies'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof((manifest -> 'author'::text)) = 'string'::text) AND (length(btrim((manifest ->> 'author'::text), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0)) AND ((jsonb_typeof((manifest -> 'permissions'::text)) = 'array'::text) AND (NOT jsonb_path_exists((manifest -> 'permissions'::text), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof((manifest -> 'dependencies'::text)) = 'array'::text) AND (NOT jsonb_path_exists((manifest -> 'dependencies'::text), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true))) AND
						CASE
						WHEN (jsonb_typeof((manifest -> 'entity'::text)) = 'object'::text) THEN (((manifest -> 'entity'::text) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'id'::text)) = 'string'::text) AND (((manifest -> 'entity'::text) ->> 'id'::text) ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text)) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'version'::text)) = 'string'::text) AND
						CASE
						WHEN (((manifest -> 'entity'::text) ->> 'version'::text) ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(((manifest -> 'entity'::text) ->> 'version'::text), '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(((manifest -> 'entity'::text) ->> 'version'::text), '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(((manifest -> 'entity'::text) ->> 'version'::text), '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
						ELSE false
						END) AND (((manifest -> 'entity'::text) ->> 'kind'::text) = ANY (ARRAY['agent'::text, 'tool'::text, 'skill'::text])) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'name'::text)) = 'object'::text) AND (((manifest -> 'entity'::text) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists(((manifest -> 'entity'::text) -> 'name'::text), 'strict $.*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'description'::text)) = 'object'::text) AND (((manifest -> 'entity'::text) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists(((manifest -> 'entity'::text) -> 'description'::text), 'strict $.*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'schema'::text)) = 'object'::text) AND public.jsonschema_is_valid((((manifest -> 'entity'::text) -> 'schema'::text))::json) AND (jsonb_typeof(((manifest -> 'entity'::text) -> 'config'::text)) = 'object'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (((NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'core_capabilities'::text)) OR (
						CASE
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text)) = 'object'::text) THEN (((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) - ARRAY['files'::text, 'shell'::text, 'python'::text, 'patch'::text, 'skills'::text, 'sharing'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'files'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'files'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'shell'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'shell'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'python'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'python'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'patch'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'patch'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'skills'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'skills'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'sharing'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'sharing'::text)) = 'boolean'::text)))) AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'skill_attachments'::text)) THEN true
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_attachments'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_attachments'::text)) <= 16)
						ELSE false
						END AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'skill_roots'::text)) THEN true
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text)) <= 8)
						ELSE false
						END AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'reference_attachments'::text)) THEN true
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'reference_attachments'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'reference_attachments'::text)) <= 8)
						ELSE false
						END AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (
						CASE
						WHEN (jsonb_typeof(((manifest -> 'entity'::text) -> 'config'::text)) = 'object'::text) THEN ((((manifest -> 'entity'::text) -> 'config'::text) - ARRAY['model'::text, 'instructions'::text, 'tools'::text, 'skills'::text, 'cluster'::text, 'max_steps'::text, 'knowledge_digest'::text, 'core_capabilities'::text, 'skill_attachments'::text, 'skill_roots'::text, 'reference_attachments'::text, 'allow_task_creation'::text, 'allow_task_delegation'::text, 'allow_memory_write'::text, 'allow_workspace_retrieval'::text, 'allow_cross_conversation_memory'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text)) = 'object'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text) -> 'version'::text)) = 'string'::text) AND (((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((length(btrim(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) ->> 'instructions'::text), ''::text), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) OR
						CASE
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text)) > 0)
						ELSE false
						END) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text]))) OR ((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_attachments'::text), '[]'::jsonb) <> '[]'::jsonb) OR (COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb) <> '[]'::jsonb)))) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'max_steps'::text)) THEN true
						WHEN ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'max_steps'::text)) = 'number'::text) AND ((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text))::numeric >= (1)::numeric) AND (((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text))::numeric <= (1000)::numeric))
						ELSE false
						END AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_task_creation'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_task_creation'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_task_delegation'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_task_delegation'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_memory_write'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_memory_write'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_workspace_retrieval'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_workspace_retrieval'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_cross_conversation_memory'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_cross_conversation_memory'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'skill'::text) OR ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text)) = 'string'::text) AND (length(btrim((((manifest -> 'entity'::text) -> 'config'::text) ->> 'instructions'::text), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'tool'::text) OR COALESCE(public.aidash_tool_config_is_valid(((manifest -> 'entity'::text) -> 'config'::text)), false))), false)"#
				                .to_owned() }, Constraint::Check { name : "packages_semver".to_owned(),
				                expression :
				                r#"COALESCE(((version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) AND
						CASE
						WHEN (version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(version, '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(version, '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(version, '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
						ELSE false
						END), false)"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
				            name: "registry".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("version", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("metadata", FieldType::Jsonb).with_not_null(true)
				            ],
				            constraints: vec![
				                Constraint::Check { name : "registry_agent_config".to_owned(), expression
				                :
				                r#"COALESCE((((kind <> 'agent'::text) OR ((((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((length(btrim(COALESCE(((metadata -> 'config'::text) ->> 'instructions'::text), ''::text), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) OR
						CASE
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skills'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skills'::text)) > 0)
						ELSE false
						END) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text]))) OR ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((COALESCE(((metadata -> 'config'::text) -> 'skill_attachments'::text), '[]'::jsonb) <> '[]'::jsonb) OR (COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb) <> '[]'::jsonb)))) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'max_steps'::text)) THEN true
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_steps'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'max_steps'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((metadata -> 'config'::text) ->> 'max_steps'::text))::numeric >= (1)::numeric) AND ((((metadata -> 'config'::text) ->> 'max_steps'::text))::numeric <= (1000)::numeric))
						ELSE false
						END)) AND ((kind <> 'agent'::text) OR (((NOT ((metadata -> 'config'::text) ? 'core_capabilities'::text)) OR (
						CASE
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'core_capabilities'::text)) = 'object'::text) THEN ((((metadata -> 'config'::text) -> 'core_capabilities'::text) - ARRAY['files'::text, 'shell'::text, 'python'::text, 'patch'::text, 'skills'::text, 'sharing'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'files'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'files'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'shell'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'shell'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'python'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'python'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'patch'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'patch'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'skills'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'skills'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'sharing'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'sharing'::text)) = 'boolean'::text)))) AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'skill_attachments'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skill_attachments'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skill_attachments'::text)) <= 16)
						ELSE false
						END AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'skill_roots'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skill_roots'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skill_roots'::text)) <= 8)
						ELSE false
						END AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'reference_attachments'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'reference_attachments'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'reference_attachments'::text)) <= 8)
						ELSE false
						END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)))) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_task_creation'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_task_creation'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_task_delegation'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_task_delegation'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_memory_write'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_memory_write'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_workspace_retrieval'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_workspace_retrieval'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_cross_conversation_memory'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_cross_conversation_memory'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['model'::text, 'instructions'::text, 'tools'::text, 'skills'::text, 'cluster'::text, 'max_steps'::text, 'knowledge_digest'::text, 'core_capabilities'::text, 'skill_attachments'::text, 'skill_roots'::text, 'reference_attachments'::text, 'allow_task_creation'::text, 'allow_task_delegation'::text, 'allow_memory_write'::text, 'allow_workspace_retrieval'::text, 'allow_cross_conversation_memory'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((kind <> 'agent'::text) OR ((jsonb_typeof((metadata #> '{config,model}'::text[])) = 'object'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'version'::text)) = 'string'::text)))), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_cluster_config"
				                .to_owned(), expression :
				                r#"COALESCE((true AND ((kind <> 'cluster'::text) OR
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['coordinator'::text]) = '{}'::jsonb)
						ELSE false
						END) AND ((kind <> 'cluster'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'coordinator'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'coordinator'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'coordinator'::text) -> 'version'::text)) = 'string'::text) AND ((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'id'::text) ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text) AND
						CASE
						WHEN ((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text) ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
						ELSE false
						END))), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_compactor_config"
				                .to_owned(), expression :
				                r#"COALESCE((true AND ((kind <> 'compactor'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'endpoint'::text, 'model'::text, 'credential_env'::text, 'max_request_bytes'::text, 'max_questions'::text, 'max_response_bytes'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(((metadata -> 'config'::text) -> 'provider'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'provider'::text) = 'typesafe-system-one'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'endpoint'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'endpoint'::text) ~ '^https?://[^/@?#[:space:]]+'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model'::text), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model'::text)) <= 128) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'credential_env'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'credential_env'::text) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text) AND
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_request_bytes'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'max_request_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'max_request_bytes'::text))::text)::numeric >= (1024)::numeric) AND (((((metadata -> 'config'::text) -> 'max_request_bytes'::text))::text)::numeric <= (1048576)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_questions'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'max_questions'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'max_questions'::text))::text)::numeric >= (1)::numeric) AND (((((metadata -> 'config'::text) -> 'max_questions'::text))::text)::numeric <= (1024)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_response_bytes'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'max_response_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'max_response_bytes'::text))::text)::numeric >= (128)::numeric) AND (((((metadata -> 'config'::text) -> 'max_response_bytes'::text))::text)::numeric <= (1048576)::numeric))
						ELSE false
						END))), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_embedding_config"
				                .to_owned(), expression :
				                r#"COALESCE((true AND ((kind <> 'embedding'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text, 'model'::text, 'model_version'::text, 'dimensions'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(((metadata -> 'config'::text) -> 'provider'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'provider'::text) = 'openai'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'endpoint'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'endpoint'::text) ~ '^https?://[^/@?#[:space:]]+'::text) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'credential_env'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'credential_env'::text), 'null'::jsonb)) <> 'string'::text) OR (((metadata -> 'config'::text) ->> 'credential_env'::text) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text)) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model'::text), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model'::text)) <= 256) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model_version'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model_version'::text), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model_version'::text)) <= 128) AND
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'dimensions'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'dimensions'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'dimensions'::text))::text)::numeric >= (1)::numeric) AND (((((metadata -> 'config'::text) -> 'dimensions'::text))::text)::numeric <= (8192)::numeric))
						ELSE false
						END))), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_identity".to_owned(),
				                expression :
				                r#"COALESCE(((id ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text) AND ((metadata -> 'id'::text) = to_jsonb(id)) AND ((metadata -> 'version'::text) = to_jsonb(version)) AND ((metadata -> 'kind'::text) = to_jsonb(kind)) AND ((kind <> 'agent'::text) OR ((octet_length(id) + octet_length(version)) <= 139))), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_kind_check"
				                .to_owned(), expression :
				                r#"(kind = ANY (ARRAY['agent'::text, 'model'::text, 'tool'::text, 'skill'::text, 'cluster'::text, 'node'::text, 'compactor'::text, 'embedding'::text]))"#
				                .to_owned() }, Constraint::Check { name : "registry_metadata_shape"
				                .to_owned(), expression :
				                r#"((jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'name'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."name".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'description'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."description".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'config'::text)) = 'object'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb)) = 'object'::text) AND public.jsonschema_is_valid((COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb))::json) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND
						CASE
						WHEN (jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) THEN (((metadata - 'installation'::text) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((NOT (metadata ? 'installation'::text)) OR ((jsonb_typeof((metadata -> 'installation'::text)) = 'object'::text) AND (((metadata -> 'installation'::text) ->> 'contract'::text) = '1'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'tenant'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'installation'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'revision'::text)) = 'number'::text))))"#
				                .to_owned() }, Constraint::Check { name : "registry_model_config"
				                .to_owned(), expression :
				                r#"COALESCE((((kind <> 'model'::text) OR (((metadata #>> '{config,provider}'::text[]) = 'openrouter'::text) AND (jsonb_typeof((metadata #> '{config,model_id}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,model_id}'::text[]), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND public.aidash_valid_http_endpoint((metadata #> '{config,endpoint}'::text[])) AND
						CASE
						WHEN (jsonb_typeof((metadata #> '{config,context_window}'::text[])) = 'number'::text) THEN ((((metadata #>> '{config,context_window}'::text[]))::numeric >= (2048)::numeric) AND (trunc(((metadata #>> '{config,context_window}'::text[]))::numeric) = ((metadata #>> '{config,context_window}'::text[]))::numeric))
						ELSE false
						END AND (jsonb_typeof((metadata #> '{config,modalities}'::text[])) = 'array'::text) AND ((metadata #> '{config,modalities}'::text[]) @> '["text"]'::jsonb) AND ((NOT ((metadata -> 'config'::text) ? 'reasoning_effort'::text)) OR ((metadata #> '{config,reasoning_effort}'::text[]) = 'null'::jsonb) OR ((metadata #>> '{config,reasoning_effort}'::text[]) = ANY (ARRAY['none'::text, 'minimal'::text, 'low'::text, 'medium'::text, 'high'::text, 'xhigh'::text, 'max'::text]))))) AND ((kind <> 'model'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'model_id'::text, 'endpoint'::text, 'credential_env'::text, 'reasoning_effort'::text, 'context_window'::text, 'max_output_tokens'::text, 'modalities'::text, 'cost'::text, 'request_timeout_secs'::text, 'media_routes'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((metadata -> 'config'::text) ? 'cost'::text) AND (jsonb_typeof(COALESCE((metadata #> '{config,credential_env}'::text[]), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND (jsonb_typeof((metadata #> '{config,modalities}'::text[])) = 'array'::text) AND (NOT jsonb_path_exists((metadata #> '{config,modalities}'::text[]), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND
						CASE
						WHEN ((jsonb_typeof((metadata #> '{config,context_window}'::text[])) = 'number'::text) AND (((metadata #> '{config,context_window}'::text[]))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((metadata #> '{config,context_window}'::text[]))::text)::numeric <= '18446744073709551615'::numeric)
						ELSE false
						END AND
						CASE
						WHEN ((NOT ((metadata -> 'config'::text) ? 'max_output_tokens'::text)) OR (((metadata -> 'config'::text) -> 'max_output_tokens'::text) = 'null'::jsonb)) THEN true
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_output_tokens'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'max_output_tokens'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'context_window'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'context_window'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((metadata -> 'config'::text) ->> 'max_output_tokens'::text))::numeric >= (1)::numeric) AND ((((metadata -> 'config'::text) ->> 'max_output_tokens'::text))::numeric <= LEAST(('4294967295'::bigint)::numeric, (((metadata -> 'config'::text) ->> 'context_window'::text))::numeric)))
						ELSE false
						END
						ELSE false
						END)) AND ((kind <> 'model'::text) OR
						CASE
						WHEN (((metadata #> '{config,request_timeout_secs}'::text[]) IS NULL) OR ((metadata #> '{config,request_timeout_secs}'::text[]) = 'null'::jsonb)) THEN true
						WHEN ((jsonb_typeof((metadata #> '{config,request_timeout_secs}'::text[])) = 'number'::text) AND (((metadata #> '{config,request_timeout_secs}'::text[]))::text ~ '^[1-9][0-9]*$'::text)) THEN (((((metadata #> '{config,request_timeout_secs}'::text[]))::text)::numeric >= (1)::numeric) AND ((((metadata #> '{config,request_timeout_secs}'::text[]))::text)::numeric <= ('4294967295'::bigint)::numeric))
						ELSE false
						END) AND ((kind <> 'model'::text) OR ((NOT ((metadata -> 'config'::text) ? 'media_routes'::text)) OR public.aidash_media_routes_valid((metadata #> '{config,media_routes}'::text[]))))), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_semver".to_owned(),
				                expression :
				                r#"COALESCE(((version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) AND
						CASE
						WHEN (version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(version, '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(version, '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(version, '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
						ELSE false
						END), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_skill_config"
				                .to_owned(), expression :
				                r#"COALESCE(((kind <> 'skill'::text) OR ((jsonb_typeof((metadata #> '{config,instructions}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,instructions}'::text[]), E'\u0009\u000a\u000b\u000c\u000a \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0))), false)"#
				                .to_owned() }, Constraint::Check { name : "registry_tool_config"
				                .to_owned(), expression :
				                r#"COALESCE((true AND ((kind <> 'tool'::text) OR public.aidash_tool_config_is_valid((metadata -> 'config'::text)))), false)"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "registry_agent_model_refs".to_owned(),
			columns: vec![
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("model_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("model_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("model_kind", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"'model'::text"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "registry_agent_model_refs_model_kind_check".to_owned(),
				expression: r#"(model_kind = 'model'::text)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "registry_agent_resource_refs".to_owned(),
			columns: vec![
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("required_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("ordinal", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("reference_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reference_version", FieldType::Text).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "registry_agent_resource_refs_required_kind_check".to_owned(),
				expression:
					r#"(required_kind = ANY (ARRAY['tool'::text, 'skill'::text, 'cluster'::text]))"#
						.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "registry_requests".to_owned(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("request", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("entity_id", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
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
