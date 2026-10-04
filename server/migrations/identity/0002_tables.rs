// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0002_tables", "identity")
        .add_dependency("federation", "0002_tables")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_bundles (
    tenant text NOT NULL,
    revision bigint NOT NULL,
    document jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT authorization_bundles_revision_check CHECK ((revision > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_catalog (
    tenant text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT authorization_catalog_revision_check CHECK ((revision > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_catalog_history (
    tenant text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    revision bigint NOT NULL,
    enabled boolean NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_credentials (
    id uuid NOT NULL,
    tenant text NOT NULL,
    subject text NOT NULL,
    token_hash bytea NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    issued_by text NOT NULL,
    CONSTRAINT authorization_credentials_check CHECK ((expires_at > created_at)),
    CONSTRAINT authorization_credentials_token_hash_check CHECK ((octet_length(token_hash) = 32))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_decisions (
    sequence bigint NOT NULL,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    subject text NOT NULL,
    action text NOT NULL,
    resource_kind text NOT NULL,
    resource_id text NOT NULL,
    decision jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE public.authorization_decisions ALTER COLUMN sequence ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.authorization_decisions_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_execution (
    run_id uuid NOT NULL,
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT authorization_execution_subject_chain_check CHECK (((cardinality(subject_chain) >= 2) AND (cardinality(subject_chain) <= 32)))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_graph_operator_grants (
    source_node text NOT NULL,
    source_operator uuid NOT NULL,
    tenant text NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT authorization_graph_operator_grants_revision_check CHECK ((revision > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_remote_commands (
    grant_id uuid NOT NULL,
    request_key text NOT NULL,
    digest text NOT NULL,
    result jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_remote_execution (
    grant_id uuid NOT NULL,
    admission_id uuid NOT NULL,
    task_id uuid NOT NULL,
    task_revision bigint NOT NULL,
    initial_task jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_remote_outputs (
    grant_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_revisions (
    tenant text NOT NULL,
    revision bigint NOT NULL,
    document jsonb NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT authorization_revisions_positive CHECK (COALESCE((revision > 0), false))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_run_outputs (
    run_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_run_reads (
    run_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL,
    CONSTRAINT authorization_run_reads_resource_kind_check CHECK ((resource_kind = ANY (ARRAY['task'::text, 'artifact'::text, 'message'::text, 'run'::text, 'conversation'::text, 'generation'::text, 'workspace_events'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_run_registry_reads (
    run_id uuid NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_run_remote_reads (
    run_id uuid NOT NULL,
    node_id text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    digest text NOT NULL,
    metadata jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_task_origins (
    task_id uuid NOT NULL,
    source_run_id uuid NOT NULL,
    tenant text NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    CONSTRAINT authorization_task_origins_subject_chain_check CHECK (((cardinality(subject_chain) >= 2) AND (cardinality(subject_chain) <= 32)))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_workspaces (
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    owner_subject text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_execution_origins (
    run_id uuid NOT NULL,
    identity_id uuid NOT NULL,
    mapping_id uuid NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_identities (
    id uuid NOT NULL,
    issuer text NOT NULL,
    subject text NOT NULL,
    last_valid_at timestamp with time zone,
    disabled_at timestamp with time zone
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_login_transactions (
    state_hash bytea NOT NULL,
    browser_hash bytea NOT NULL,
    nonce text NOT NULL,
    pkce_verifier text NOT NULL,
    return_to text NOT NULL,
    callback_uri text NOT NULL,
    expires_at timestamp with time zone NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_logout_tokens (
    jti_hash bytea NOT NULL,
    expires_at timestamp with time zone NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_mappings (
    id uuid NOT NULL,
    identity_id uuid NOT NULL,
    tenant text NOT NULL,
    subject text NOT NULL,
    credential_id uuid NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_operator_grants (
    identity_id uuid NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_registration_requests (
    id uuid NOT NULL,
    identity_id uuid NOT NULL,
    status text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    decided_at timestamp with time zone,
    decided_by uuid,
    decision_actor text
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.dashboard_sessions (
    id uuid NOT NULL,
    token_hash bytea NOT NULL,
    csrf_hash bytea NOT NULL,
    identity_id uuid NOT NULL,
    provider_sid text,
    created_at timestamp with time zone NOT NULL,
    last_activity_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone
);"#.to_string(),
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
