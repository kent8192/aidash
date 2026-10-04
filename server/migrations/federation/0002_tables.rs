// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0002_tables", "federation")
        .add_dependency("execution", "0002_tables")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_authority_attempts (
    transaction_id uuid NOT NULL,
    node_id text NOT NULL,
    outcome text
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_coordinators (
    id uuid NOT NULL,
    digest text NOT NULL,
    manifest jsonb NOT NULL,
    decision text,
    visible boolean DEFAULT false NOT NULL,
    complete boolean DEFAULT false NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT atomic_coordinators_check CHECK (((NOT visible) OR ((decision IS NOT NULL) AND (decision = 'COMMIT'::text)))),
    CONSTRAINT atomic_coordinators_check1 CHECK (((NOT complete) OR (decision IS NOT NULL))),
    CONSTRAINT atomic_coordinators_decision_check CHECK ((decision = ANY (ARRAY['COMMIT'::text, 'ABORT'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_gate (
    singleton boolean DEFAULT true NOT NULL,
    transaction_id uuid,
    commit_epoch bigint DEFAULT 0 NOT NULL,
    CONSTRAINT atomic_gate_singleton_check CHECK ((singleton = true))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_history (
    sequence bigint NOT NULL,
    transaction_id uuid NOT NULL,
    role text NOT NULL,
    phase text NOT NULL,
    detail text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT atomic_history_role_check CHECK ((role = ANY (ARRAY['coordinator'::text, 'participant'::text, 'trust'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE SEQUENCE public.atomic_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER SEQUENCE public.atomic_history_sequence_seq OWNED BY public.atomic_history.sequence;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_participants (
    id uuid NOT NULL,
    coordinator text NOT NULL,
    digest text NOT NULL,
    manifest jsonb NOT NULL,
    phase text NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT atomic_participants_phase_check CHECK ((phase = ANY (ARRAY['RESERVED'::text, 'PREPARED'::text, 'APPLIED'::text, 'COMMITTED'::text, 'ABORTED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_peer_trust (
    node_id text NOT NULL,
    enabled boolean NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_preflights (
    id uuid NOT NULL,
    binding jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_subjects (
    id uuid NOT NULL,
    binding jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.atomic_votes (
    transaction_id uuid NOT NULL,
    node_id text NOT NULL,
    phase text DEFAULT 'PENDING'::text NOT NULL,
    CONSTRAINT atomic_votes_phase_check CHECK ((phase = ANY (ARRAY['PENDING'::text, 'RESERVED'::text, 'PREPARED'::text, 'APPLIED'::text, 'COMMITTED'::text, 'ABORTED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_peer_mapping_history (
    sequence bigint NOT NULL,
    source_node text NOT NULL,
    source_tenant text NOT NULL,
    source_subject text NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    actor text NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT authorization_peer_mapping_history_revision_check CHECK ((revision > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE SEQUENCE public.authorization_peer_mapping_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER SEQUENCE public.authorization_peer_mapping_history_sequence_seq OWNED BY public.authorization_peer_mapping_history.sequence;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_peer_mappings (
    source_node text NOT NULL,
    source_tenant text NOT NULL,
    source_subject text NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    actor text NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT authorization_peer_mappings_revision_check CHECK ((revision > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_remote_admissions (
    id uuid NOT NULL,
    source_node text NOT NULL,
    grant_id uuid NOT NULL,
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    subject_chain text[] NOT NULL,
    description jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_remote_grant_reads (
    grant_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.authorization_remote_grants (
    id uuid NOT NULL,
    task_id uuid NOT NULL,
    task_revision bigint NOT NULL,
    workspace_id uuid NOT NULL,
    node_id text NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    inspection jsonb NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked boolean DEFAULT false NOT NULL,
    semantic jsonb DEFAULT '{"mode": "disabled"}'::jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.delegations (
    task_id uuid NOT NULL,
    node_id text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    delivered boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.peer_events (
    node_id text NOT NULL,
    event_id uuid NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.peers (
    node_id text NOT NULL,
    endpoint text NOT NULL,
    credential_env text NOT NULL,
    protocol_version text NOT NULL,
    enabled boolean DEFAULT true NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.remote_run_message_fences (
    task_id uuid NOT NULL,
    run_id uuid NOT NULL,
    idempotency_key text NOT NULL,
    content text NOT NULL,
    input_seq bigint,
    expires_at timestamp with time zone,
    consumed boolean DEFAULT false NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_history ALTER COLUMN sequence SET DEFAULT nextval('public.atomic_history_sequence_seq'::regclass);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_peer_mapping_history ALTER COLUMN sequence SET DEFAULT nextval('public.authorization_peer_mapping_history_sequence_seq'::regclass);"#.to_string(),
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
