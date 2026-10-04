// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0002_tables", "registry")
        .add_dependency("marketplace", "0002_tables")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_draft_registrations (
    draft_id uuid NOT NULL,
    revision bigint NOT NULL,
    agent_id text NOT NULL,
    version text NOT NULL,
    actor text NOT NULL,
    release_notes text DEFAULT ''::text NOT NULL,
    source_id text,
    source_version text,
    behavioral_tested boolean DEFAULT false NOT NULL,
    registered_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_draft_shares (
    draft_id uuid NOT NULL,
    subject text NOT NULL,
    can_edit boolean DEFAULT false NOT NULL,
    documents_digest text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_drafts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    owner text NOT NULL,
    managed_id text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    entry jsonb NOT NULL,
    documents jsonb NOT NULL,
    release_notes text DEFAULT ''::text NOT NULL,
    source_id text,
    source_version text,
    archived boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_incident_events (
    id bigint NOT NULL,
    incident_id uuid NOT NULL,
    actor text NOT NULL,
    change jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE SEQUENCE public.agent_incident_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER SEQUENCE public.agent_incident_events_id_seq OWNED BY public.agent_incident_events.id;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_incidents (
    id uuid NOT NULL,
    tenant text NOT NULL,
    agent_id text NOT NULL,
    version text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    severity text NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    owner text NOT NULL,
    notes text NOT NULL,
    evidence jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    resolved_at timestamp with time zone,
    evidence_expires_at timestamp with time zone,
    evidence_expired_at timestamp with time zone
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_knowledge (
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    documents jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_test_limits (
    tenant text NOT NULL,
    max_input_bytes integer DEFAULT 20000 NOT NULL,
    max_output_tokens integer DEFAULT 2048 NOT NULL,
    max_total_tokens integer DEFAULT 16384 NOT NULL,
    max_steps integer DEFAULT 12 NOT NULL,
    max_duration_secs integer DEFAULT 60 NOT NULL,
    max_concurrent integer DEFAULT 2 NOT NULL,
    payload_days integer DEFAULT 30 NOT NULL,
    incident_evidence_days integer DEFAULT 90 NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_test_profiles (
    tenant text NOT NULL,
    id text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    rules jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.agent_test_sessions (
    id uuid NOT NULL,
    draft_id uuid NOT NULL,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    status text NOT NULL,
    scenario jsonb NOT NULL,
    conversation jsonb,
    tool_calls jsonb,
    usage jsonb NOT NULL,
    error text,
    active_slot uuid,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    expired_at timestamp with time zone
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.installations (
    id text NOT NULL,
    version text NOT NULL,
    digest text NOT NULL,
    config jsonb NOT NULL,
    installed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT installations_config CHECK (COALESCE((jsonb_typeof(config) = 'object'::text), false))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.packages (
    id text NOT NULL,
    version text NOT NULL,
    manifest jsonb NOT NULL,
    digest text NOT NULL,
    manifest_source text DEFAULT ''::text NOT NULL,
    CONSTRAINT packages_digest CHECK (COALESCE((public.aidash_package_source_matches(manifest, manifest_source) AND (digest = ('sha256:'::text || encode(sha256(convert_to(manifest_source, 'UTF8'::name)), 'hex'::text)))), false)),
    CONSTRAINT packages_identity CHECK (COALESCE(((jsonb_typeof(manifest) = 'object'::text) AND ((manifest #> '{entity,id}'::text[]) = to_jsonb(id)) AND ((manifest #> '{entity,version}'::text[]) = to_jsonb(version)) AND
CASE
    WHEN (jsonb_typeof(manifest) = 'object'::text) THEN ((manifest - ARRAY['entity'::text, 'author'::text, 'permissions'::text, 'dependencies'::text]) = '{}'::jsonb)
    ELSE false
END AND ((jsonb_typeof((manifest -> 'author'::text)) = 'string'::text) AND (length(btrim((manifest ->> 'author'::text), '	

                  　'::text)) > 0)) AND ((jsonb_typeof((manifest -> 'permissions'::text)) = 'array'::text) AND (NOT jsonb_path_exists((manifest -> 'permissions'::text), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof((manifest -> 'dependencies'::text)) = 'array'::text) AND (NOT jsonb_path_exists((manifest -> 'dependencies'::text), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true))) AND
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
END AND (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text)) = 'object'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text) -> 'version'::text)) = 'string'::text) AND (((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((length(btrim(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) ->> 'instructions'::text), ''::text), '	

                  　'::text)) > 0) OR
CASE
    WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text)) > 0)
    ELSE false
END) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text]))) OR ((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_attachments'::text), '[]'::jsonb) <> '[]'::jsonb) OR (COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb) <> '[]'::jsonb)))) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND
CASE
    WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'max_steps'::text)) THEN true
    WHEN ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'max_steps'::text)) = 'number'::text) AND ((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text))::numeric >= (1)::numeric) AND (((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text))::numeric <= (1000)::numeric))
    ELSE false
END AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_task_creation'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_task_creation'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_task_delegation'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_task_delegation'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_memory_write'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_memory_write'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_workspace_retrieval'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_workspace_retrieval'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_cross_conversation_memory'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_cross_conversation_memory'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'skill'::text) OR ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text)) = 'string'::text) AND (length(btrim((((manifest -> 'entity'::text) -> 'config'::text) ->> 'instructions'::text), '	

                  　'::text)) > 0))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'tool'::text) OR COALESCE(public.aidash_tool_config_is_valid(((manifest -> 'entity'::text) -> 'config'::text)), false))), false)),
    CONSTRAINT packages_semver CHECK (COALESCE(((version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) AND
CASE
    WHEN (version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(version, '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(version, '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(version, '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
    ELSE false
END), false))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.registry (
    id text NOT NULL,
    version text NOT NULL,
    kind text NOT NULL,
    metadata jsonb NOT NULL,
    CONSTRAINT registry_agent_config CHECK (COALESCE((((kind <> 'agent'::text) OR ((((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((length(btrim(COALESCE(((metadata -> 'config'::text) ->> 'instructions'::text), ''::text), '	

                  　'::text)) > 0) OR
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
END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((kind <> 'agent'::text) OR ((jsonb_typeof((metadata #> '{config,model}'::text[])) = 'object'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'version'::text)) = 'string'::text)))), false)),
    CONSTRAINT registry_cluster_config CHECK (COALESCE((true AND ((kind <> 'cluster'::text) OR
CASE
    WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['coordinator'::text]) = '{}'::jsonb)
    ELSE false
END) AND ((kind <> 'cluster'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'coordinator'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'coordinator'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'coordinator'::text) -> 'version'::text)) = 'string'::text) AND ((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'id'::text) ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text) AND
CASE
    WHEN ((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text) ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
    ELSE false
END))), false)),
    CONSTRAINT registry_compactor_config CHECK (COALESCE((true AND ((kind <> 'compactor'::text) OR (
CASE
    WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'endpoint'::text, 'model'::text, 'credential_env'::text, 'max_request_bytes'::text, 'max_questions'::text, 'max_response_bytes'::text]) = '{}'::jsonb)
    ELSE false
END AND (jsonb_typeof(((metadata -> 'config'::text) -> 'provider'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'provider'::text) = 'typesafe-system-one'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'endpoint'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'endpoint'::text) ~ '^https?://[^/@?#[:space:]]+'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model'::text), '	

                  　'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model'::text)) <= 128) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'credential_env'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'credential_env'::text) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text) AND
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
END))), false)),
    CONSTRAINT registry_embedding_config CHECK (COALESCE((true AND ((kind <> 'embedding'::text) OR (
CASE
    WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text, 'model'::text, 'model_version'::text, 'dimensions'::text]) = '{}'::jsonb)
    ELSE false
END AND (jsonb_typeof(((metadata -> 'config'::text) -> 'provider'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'provider'::text) = 'openai'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'endpoint'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'endpoint'::text) ~ '^https?://[^/@?#[:space:]]+'::text) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'credential_env'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'credential_env'::text), 'null'::jsonb)) <> 'string'::text) OR (((metadata -> 'config'::text) ->> 'credential_env'::text) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text)) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model'::text), '	

                  　'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model'::text)) <= 256) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model_version'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model_version'::text), '	

                  　'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model_version'::text)) <= 128) AND
CASE
    WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'dimensions'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'dimensions'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'dimensions'::text))::text)::numeric >= (1)::numeric) AND (((((metadata -> 'config'::text) -> 'dimensions'::text))::text)::numeric <= (8192)::numeric))
    ELSE false
END))), false)),
    CONSTRAINT registry_identity CHECK (COALESCE(((id ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text) AND ((metadata -> 'id'::text) = to_jsonb(id)) AND ((metadata -> 'version'::text) = to_jsonb(version)) AND ((metadata -> 'kind'::text) = to_jsonb(kind)) AND ((kind <> 'agent'::text) OR ((octet_length(id) + octet_length(version)) <= 139))), false)),
    CONSTRAINT registry_kind_check CHECK ((kind = ANY (ARRAY['agent'::text, 'model'::text, 'tool'::text, 'skill'::text, 'cluster'::text, 'node'::text, 'compactor'::text, 'embedding'::text]))),
    CONSTRAINT registry_metadata_shape CHECK (((jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'name'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."name".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'description'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."description".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'config'::text)) = 'object'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb)) = 'object'::text) AND public.jsonschema_is_valid((COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb))::json) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND
CASE
    WHEN (jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) THEN (((metadata - 'installation'::text) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
    ELSE false
END AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((NOT (metadata ? 'installation'::text)) OR ((jsonb_typeof((metadata -> 'installation'::text)) = 'object'::text) AND (((metadata -> 'installation'::text) ->> 'contract'::text) = '1'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'tenant'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'installation'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'revision'::text)) = 'number'::text))))),
    CONSTRAINT registry_model_config CHECK (COALESCE((((kind <> 'model'::text) OR (((metadata #>> '{config,provider}'::text[]) = 'openrouter'::text) AND (jsonb_typeof((metadata #> '{config,model_id}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,model_id}'::text[]), '	

                  　'::text)) > 0) AND public.aidash_valid_http_endpoint((metadata #> '{config,endpoint}'::text[])) AND
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
END) AND ((kind <> 'model'::text) OR ((NOT ((metadata -> 'config'::text) ? 'media_routes'::text)) OR public.aidash_media_routes_valid((metadata #> '{config,media_routes}'::text[]))))), false)),
    CONSTRAINT registry_semver CHECK (COALESCE(((version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) AND
CASE
    WHEN (version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(version, '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(version, '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(version, '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
    ELSE false
END), false)),
    CONSTRAINT registry_skill_config CHECK (COALESCE(((kind <> 'skill'::text) OR ((jsonb_typeof((metadata #> '{config,instructions}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,instructions}'::text[]), '	

                  　'::text)) > 0))), false)),
    CONSTRAINT registry_tool_config CHECK (COALESCE((true AND ((kind <> 'tool'::text) OR public.aidash_tool_config_is_valid((metadata -> 'config'::text)))), false))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.registry_agent_model_refs (
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    model_id text NOT NULL,
    model_version text NOT NULL,
    model_kind text DEFAULT 'model'::text NOT NULL,
    CONSTRAINT registry_agent_model_refs_model_kind_check CHECK ((model_kind = 'model'::text))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.registry_agent_resource_refs (
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    required_kind text NOT NULL,
    ordinal integer NOT NULL,
    reference_id text NOT NULL,
    reference_version text NOT NULL,
    CONSTRAINT registry_agent_resource_refs_required_kind_check CHECK ((required_kind = ANY (ARRAY['tool'::text, 'skill'::text, 'cluster'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.registry_requests (
    key uuid NOT NULL,
    request jsonb NOT NULL,
    entity_id text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_incident_events ALTER COLUMN id SET DEFAULT nextval('public.agent_incident_events_id_seq'::regclass);"#.to_string(),
            // Refuse irreversible baseline rollback before the native ledger changes.
            reverse_sql: Some(r#"DO $aidash_baseline$
BEGIN
    RAISE EXCEPTION 'Aidash frozen baseline is forward-only; restore a backup to roll back';
END
$aidash_baseline$;"#.to_string()),
        })
        .atomic(true)
        .database_only(true)
}
