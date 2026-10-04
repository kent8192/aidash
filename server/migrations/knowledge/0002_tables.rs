// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0002_tables", "knowledge")
        .add_dependency("identity", "0002_tables")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_agent_memory (
    entry_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    home_node text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_collections (
    collection text NOT NULL,
    workspace_id uuid NOT NULL,
    vector jsonb NOT NULL,
    retired boolean DEFAULT false NOT NULL,
    last_error text,
    cleaned_at timestamp with time zone,
    next_attempt timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_entries (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    key text NOT NULL,
    source jsonb NOT NULL,
    agent text,
    metadata jsonb NOT NULL,
    revision bigint NOT NULL,
    point_id uuid NOT NULL,
    index_revision bigint NOT NULL,
    deleted boolean DEFAULT false NOT NULL,
    state text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    created_by text NOT NULL,
    authority jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    next_attempt timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT semantic_entries_authority CHECK (COALESCE(((jsonb_typeof(authority) = 'object'::text) AND (authority ? 'credential'::text) AND (jsonb_typeof((authority -> 'credential'::text)) = ANY (ARRAY['string'::text, 'null'::text])) AND (authority ? 'tenant'::text) AND (jsonb_typeof((authority -> 'tenant'::text)) = 'string'::text) AND (authority ? 'subject'::text) AND (jsonb_typeof((authority -> 'subject'::text)) = 'string'::text) AND (authority ? 'subjects'::text) AND (jsonb_typeof((authority -> 'subjects'::text)) = 'array'::text) AND (NOT jsonb_path_exists(authority, 'strict $."subjects"[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (((authority -> 'credential'::text) = 'null'::jsonb) OR ((authority ->> 'credential'::text) ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'::text))), false)),
    CONSTRAINT semantic_entries_counters CHECK (COALESCE(((revision > 0) AND (index_revision > 0) AND (attempts >= 0) AND
CASE (source ->> 'kind'::text)
    WHEN 'memory'::text THEN (
    CASE
        WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'text'::text]) = '{}'::jsonb)
        ELSE false
    END AND (jsonb_typeof((source -> 'text'::text)) = 'string'::text) AND (deleted OR (length(btrim((source ->> 'text'::text), '	

                  　'::text)) > 0)))
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
END), false)),
    CONSTRAINT semantic_entries_state_check CHECK ((state = ANY (ARRAY['PENDING'::text, 'READY'::text, 'ERROR'::text, 'REVOKED'::text, 'DELETED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_history (
    sequence bigint NOT NULL,
    workspace_id uuid NOT NULL,
    entry_id uuid,
    revision bigint NOT NULL,
    state text NOT NULL,
    detail text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE SEQUENCE public.semantic_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER SEQUENCE public.semantic_history_sequence_seq OWNED BY public.semantic_history.sequence;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_indexes (
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    spec jsonb NOT NULL,
    collection text NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT semantic_indexes_revision CHECK (COALESCE(((revision > 0) AND (revision < '9223372036854775807'::bigint) AND (jsonb_typeof(spec) = 'object'::text) AND
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
END AND ((spec #>> '{embedding,provider}'::text[]) = 'openai'::text) AND public.aidash_valid_http_endpoint((spec #> '{embedding,endpoint}'::text[])) AND (((spec #> '{embedding,credential_env}'::text[]) IS NULL) OR ((spec #> '{embedding,credential_env}'::text[]) = 'null'::jsonb) OR ((jsonb_typeof((spec #> '{embedding,credential_env}'::text[])) = 'string'::text) AND ((spec #>> '{embedding,credential_env}'::text[]) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text))) AND ((spec #>> '{vector,provider}'::text[]) = 'qdrant'::text) AND public.aidash_valid_http_endpoint((spec #> '{vector,endpoint}'::text[])) AND (((spec #> '{vector,credential_env}'::text[]) IS NULL) OR ((spec #> '{vector,credential_env}'::text[]) = 'null'::jsonb) OR ((jsonb_typeof((spec #> '{vector,credential_env}'::text[])) = 'string'::text) AND ((spec #>> '{vector,credential_env}'::text[]) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text))) AND ((jsonb_typeof((spec #> '{embedding,model}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model}'::text[]), '	

                  　'::text)) > 0) AND (octet_length((spec #>> '{embedding,model}'::text[])) <= 256)) AND ((jsonb_typeof((spec #> '{embedding,model_version}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model_version}'::text[]), '	

                  　'::text)) > 0) AND (octet_length((spec #>> '{embedding,model_version}'::text[])) <= 128)) AND
CASE
    WHEN ((jsonb_typeof((spec #> '{embedding,dimensions}'::text[])) = 'number'::text) AND (((spec #> '{embedding,dimensions}'::text[]))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric >= (1)::numeric) AND ((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric <= (8192)::numeric))
    ELSE false
END), false))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_points (
    id uuid NOT NULL,
    entry_id uuid NOT NULL,
    content_digest text,
    collection text NOT NULL,
    retired boolean DEFAULT false NOT NULL,
    last_error text,
    cleaned_at timestamp with time zone,
    next_attempt timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_remote_attempts (
    id uuid NOT NULL,
    operation_id uuid NOT NULL,
    fence bigint NOT NULL,
    cycle integer NOT NULL,
    state text NOT NULL,
    reservations jsonb DEFAULT '[]'::jsonb NOT NULL,
    error text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    dispatched_at timestamp with time zone,
    completed_at timestamp with time zone,
    CONSTRAINT semantic_remote_attempts_state_check CHECK ((state = ANY (ARRAY['RESERVING'::text, 'DISPATCHED'::text, 'COMPLETED'::text, 'ABORTED'::text, 'UNCERTAIN'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_remote_operations (
    id uuid NOT NULL,
    home_node text NOT NULL,
    grant_id uuid NOT NULL,
    admission_id uuid NOT NULL,
    digest text NOT NULL,
    binding jsonb NOT NULL,
    state text DEFAULT 'PENDING'::text NOT NULL,
    cycle integer DEFAULT 0 NOT NULL,
    failures integer DEFAULT 0 NOT NULL,
    attempt_id uuid,
    fence bigint DEFAULT 0 NOT NULL,
    lease_until timestamp with time zone,
    next_attempt timestamp with time zone,
    error text,
    receipt jsonb,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT semantic_remote_operations_state_check CHECK ((state = ANY (ARRAY['PENDING'::text, 'ACTIVE'::text, 'WAITING'::text, 'READY'::text, 'PAUSED'::text, 'INVALIDATED'::text, 'CANCELLED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_remote_reads (
    grant_id uuid NOT NULL,
    admission_id uuid NOT NULL,
    entry_id uuid NOT NULL,
    revision bigint NOT NULL,
    content_digest text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_remote_receipts (
    operation_id uuid NOT NULL,
    run_id uuid NOT NULL,
    digest text NOT NULL,
    receipt jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.semantic_run_reads (
    run_id uuid NOT NULL,
    entry_id uuid NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT semantic_run_reads_revision CHECK (COALESCE((revision > 0), false))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_history ALTER COLUMN sequence SET DEFAULT nextval('public.semantic_history_sequence_seq'::regclass);"#.to_string(),
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
