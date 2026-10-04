// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0002_tables", "execution")
        .add_dependency("workspaces", "0001_functions")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.activation_quarantine (
    digest text NOT NULL,
    reason text NOT NULL,
    stream_sequence bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_areas (
    id uuid NOT NULL,
    tenant text NOT NULL,
    home_node text NOT NULL,
    agent_id text NOT NULL,
    owner text NOT NULL,
    workspace_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    generation bigint DEFAULT 1 NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    epoch bigint DEFAULT 1 NOT NULL,
    next_sequence bigint DEFAULT 1 NOT NULL,
    state text DEFAULT 'active'::text NOT NULL,
    manifest jsonb DEFAULT '[]'::jsonb NOT NULL,
    constraints jsonb DEFAULT '[]'::jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_objects (
    id uuid NOT NULL,
    tenant text NOT NULL,
    area_id uuid,
    kind text NOT NULL,
    digest text NOT NULL,
    size bigint NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_operations (
    id uuid NOT NULL,
    area_id uuid NOT NULL,
    run_id uuid NOT NULL,
    credential_id uuid NOT NULL,
    tenant text NOT NULL,
    principal text NOT NULL,
    request_key text NOT NULL,
    digest text NOT NULL,
    kind text NOT NULL,
    state text NOT NULL,
    epoch bigint NOT NULL,
    generation bigint NOT NULL,
    revision bigint NOT NULL,
    policy_revision bigint NOT NULL,
    subjects jsonb NOT NULL,
    input jsonb NOT NULL,
    result jsonb NOT NULL,
    runner_instance text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_quotas (
    tenant text NOT NULL,
    used_bytes bigint DEFAULT 0 NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_records (
    id uuid NOT NULL,
    tenant text NOT NULL,
    owner text NOT NULL,
    area_id uuid,
    kind text NOT NULL,
    state text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    data jsonb NOT NULL,
    expires_at timestamp with time zone
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_requests (
    tenant text NOT NULL,
    principal text NOT NULL,
    key uuid NOT NULL,
    digest text NOT NULL,
    result jsonb NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_runs (
    run_id uuid NOT NULL,
    area_id uuid NOT NULL,
    sequence bigint NOT NULL,
    initialized boolean DEFAULT false NOT NULL,
    generation bigint DEFAULT 1 NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.core_task_sessions (
    task_id uuid NOT NULL,
    thread_id uuid NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.events (
    sequence bigint NOT NULL,
    id uuid NOT NULL,
    node_id text NOT NULL,
    workspace_id uuid,
    kind text NOT NULL,
    data jsonb NOT NULL,
    published_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    publish_error text
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE public.events ALTER COLUMN sequence ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.events_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_budgets (
    request_id uuid NOT NULL,
    token_limit bigint NOT NULL,
    used_tokens bigint DEFAULT 0 NOT NULL,
    compaction_call_limit bigint DEFAULT 0 NOT NULL,
    compaction_calls bigint DEFAULT 0 NOT NULL,
    embedding_call_limit bigint DEFAULT 0 NOT NULL,
    embedding_calls bigint DEFAULT 0 NOT NULL,
    CONSTRAINT embedding_calls_bounded CHECK ((embedding_calls <= embedding_call_limit)),
    CONSTRAINT generation_budgets_check CHECK (((used_tokens >= 0) AND (used_tokens <= token_limit))),
    CONSTRAINT generation_budgets_check1 CHECK (((compaction_calls >= 0) AND (compaction_calls <= compaction_call_limit))),
    CONSTRAINT generation_budgets_compaction_call_limit_check CHECK ((compaction_call_limit >= 0)),
    CONSTRAINT generation_budgets_embedding_call_limit_check CHECK ((embedding_call_limit >= 0)),
    CONSTRAINT generation_budgets_embedding_calls_check CHECK ((embedding_calls >= 0)),
    CONSTRAINT generation_budgets_token_limit_check CHECK ((token_limit > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_compaction_usage (
    request_id uuid NOT NULL,
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    request_bytes bigint NOT NULL,
    questions integer NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT generation_compaction_usage_questions_check CHECK ((questions > 0)),
    CONSTRAINT generation_compaction_usage_request_bytes_check CHECK ((request_bytes > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_embedding_usage (
    request_id uuid NOT NULL,
    attempt_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    run_id uuid,
    entry_id uuid,
    purpose text NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    request_bytes bigint NOT NULL,
    reserved_tokens bigint NOT NULL,
    reported_tokens bigint,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT generation_embedding_usage_purpose_check CHECK ((purpose = ANY (ARRAY['query'::text, 'index'::text]))),
    CONSTRAINT generation_embedding_usage_reported_tokens_check CHECK ((reported_tokens >= 0)),
    CONSTRAINT generation_embedding_usage_request_bytes_check CHECK ((request_bytes > 0)),
    CONSTRAINT generation_embedding_usage_reserved_tokens_check CHECK ((reserved_tokens > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_history (
    sequence bigint NOT NULL,
    request_id uuid NOT NULL,
    status text NOT NULL,
    actor text NOT NULL,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE public.generation_history ALTER COLUMN sequence ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.generation_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_policies (
    tenant text NOT NULL,
    id text NOT NULL,
    revision bigint NOT NULL,
    spec jsonb NOT NULL,
    generated_count bigint DEFAULT 0 NOT NULL,
    allocated_tokens bigint DEFAULT 0 NOT NULL,
    allocated_compaction_calls bigint DEFAULT 0 NOT NULL,
    allocated_embedding_calls bigint DEFAULT 0 NOT NULL,
    CONSTRAINT generation_policies_allocated_compaction_calls_check CHECK ((allocated_compaction_calls >= 0)),
    CONSTRAINT generation_policies_allocated_embedding_calls_check CHECK ((allocated_embedding_calls >= 0)),
    CONSTRAINT generation_policies_allocated_tokens_check CHECK ((allocated_tokens >= 0)),
    CONSTRAINT generation_policies_generated_count_check CHECK ((generated_count >= 0)),
    CONSTRAINT generation_policies_revision_check CHECK ((revision > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_policy_history (
    tenant text NOT NULL,
    policy_id text NOT NULL,
    revision bigint NOT NULL,
    spec jsonb NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT generation_policy_history_revision CHECK (COALESCE((revision > 0), false))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_remote_dispatches (
    attempt_id uuid NOT NULL,
    usage jsonb NOT NULL,
    digest text NOT NULL,
    peer_node text NOT NULL,
    boundary jsonb NOT NULL,
    state text DEFAULT 'PREPARING'::text NOT NULL,
    reservations jsonb DEFAULT '[]'::jsonb NOT NULL,
    finalization jsonb,
    peer_finalized boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT generation_remote_dispatches_state_check CHECK ((state = ANY (ARRAY['PREPARING'::text, 'DISPATCHED'::text, 'SETTLED'::text, 'ABORTED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_remote_finalizations (
    attempt_id uuid NOT NULL,
    digest text NOT NULL,
    result jsonb
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_remote_intents (
    id uuid NOT NULL,
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    binding jsonb NOT NULL,
    cancelled boolean DEFAULT false NOT NULL,
    cancel_delivered boolean DEFAULT false NOT NULL,
    cancel_retry_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_remote_usage (
    request_id uuid NOT NULL,
    attempt_id uuid NOT NULL,
    operation_id uuid NOT NULL,
    dispatcher_node text NOT NULL,
    grant_id uuid NOT NULL,
    admission_id uuid NOT NULL,
    purpose text NOT NULL,
    digest text NOT NULL,
    reserved_tokens bigint NOT NULL,
    reported_tokens bigint,
    state text DEFAULT 'RESERVED'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT generation_remote_usage_purpose_check CHECK ((purpose = ANY (ARRAY['embedding'::text, 'inference'::text, 'compaction'::text]))),
    CONSTRAINT generation_remote_usage_reserved_tokens_check CHECK ((reserved_tokens >= 0)),
    CONSTRAINT generation_remote_usage_state_check CHECK ((state = ANY (ARRAY['RESERVED'::text, 'SETTLED'::text, 'RELEASED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_requests (
    id uuid NOT NULL,
    tenant text NOT NULL,
    policy_id text NOT NULL,
    policy_revision bigint NOT NULL,
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    definition jsonb NOT NULL,
    status text NOT NULL,
    reason text NOT NULL,
    depth integer NOT NULL,
    token_limit bigint NOT NULL,
    quota_released boolean DEFAULT false NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    retired_catalog_revision bigint,
    home_node text DEFAULT ''::text NOT NULL,
    foreign_intent jsonb,
    prepared boolean DEFAULT false NOT NULL,
    grant_id uuid,
    admission_id uuid,
    local_task_id uuid GENERATED ALWAYS AS (
CASE
    WHEN (home_node = ''::text) THEN task_id
    ELSE NULL::uuid
END) STORED,
    local_workspace_id uuid GENERATED ALWAYS AS (
CASE
    WHEN (home_node = ''::text) THEN workspace_id
    ELSE NULL::uuid
END) STORED,
    CONSTRAINT generation_foreign_binding CHECK ((((home_node = ''::text) AND (foreign_intent IS NULL) AND (grant_id IS NULL) AND (admission_id IS NULL)) OR ((home_node <> ''::text) AND (foreign_intent IS NOT NULL)))),
    CONSTRAINT generation_requests_depth_check CHECK (((depth >= 1) AND (depth <= 31))),
    CONSTRAINT generation_requests_status_check CHECK ((status = ANY (ARRAY['PENDING_APPROVAL'::text, 'QUEUED'::text, 'ACTIVE'::text, 'COMPLETED'::text, 'DENIED'::text, 'STOPPED'::text, 'EXPIRED'::text, 'FAILED'::text, 'DELETED'::text]))),
    CONSTRAINT generation_requests_subject_chain_check CHECK (((cardinality(subject_chain) >= 1) AND (cardinality(subject_chain) <= 31))),
    CONSTRAINT generation_requests_token_limit_check CHECK ((token_limit > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.generation_usage (
    request_id uuid NOT NULL,
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL,
    reserved_tokens bigint NOT NULL,
    reported_tokens bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT generation_usage_reported_tokens_check CHECK ((reported_tokens >= 0)),
    CONSTRAINT generation_usage_reserved_tokens_check CHECK ((reserved_tokens > 0))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.human_requests (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    run_id uuid NOT NULL,
    kind text NOT NULL,
    prompt text NOT NULL,
    response jsonb,
    request_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    answered_by text,
    CONSTRAINT human_requests_kind_check CHECK ((kind = ANY (ARRAY['QUESTION'::text, 'APPROVAL_REQUIRED'::text, 'CONFIRMATION'::text, 'INFORMATION_REQUEST'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.inbox (
    event_id uuid NOT NULL,
    received_at timestamp with time zone DEFAULT now() NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.invocations (
    idempotency_key text NOT NULL,
    run_id uuid NOT NULL,
    tool text NOT NULL,
    input jsonb NOT NULL,
    status text NOT NULL,
    result jsonb,
    replay_safe boolean NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT invocations_status_check CHECK ((status = ANY (ARRAY['STARTED'::text, 'COMPLETED'::text, 'UNCERTAIN'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.memory (
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    workspace_id uuid NOT NULL,
    data jsonb DEFAULT '{}'::jsonb NOT NULL,
    home_node text DEFAULT ''::text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.run_activations (
    generation bigint NOT NULL,
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    run_id uuid NOT NULL,
    run_revision bigint NOT NULL,
    reason text NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    due_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    publication_epoch bigint DEFAULT 0 NOT NULL,
    publish_token uuid,
    publish_until timestamp with time zone,
    published_at timestamp with time zone,
    lease_token uuid,
    claimed_at timestamp with time zone,
    claim_source text,
    worker_pid bigint,
    disposition text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT run_activations_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'claimed'::text, 'deferred'::text, 'settled'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE SEQUENCE public.run_activations_generation_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER SEQUENCE public.run_activations_generation_seq OWNED BY public.run_activations.generation;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.run_inputs (
    seq bigint NOT NULL,
    run_id uuid NOT NULL,
    sender text NOT NULL,
    content text NOT NULL,
    message_id uuid,
    idempotency_key text NOT NULL,
    delivery_retry_at timestamp with time zone,
    reference_only boolean DEFAULT false NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE SEQUENCE public.run_inputs_seq_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER SEQUENCE public.run_inputs_seq_seq OWNED BY public.run_inputs.seq;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.runs (
    id uuid NOT NULL,
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    home_node text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    phase text DEFAULT 'READY'::text NOT NULL,
    control text DEFAULT 'ACTIVE'::text NOT NULL,
    context jsonb DEFAULT '{"usage": null, "history": [], "summary": "", "compactions": 0, "media_inferred_seq": 0, "run_message_summary": "", "message_read_coverage": {}, "run_message_summary_seq": 0, "message_inference_coverage": {}}'::jsonb NOT NULL,
    pending jsonb DEFAULT '{"data": {}, "recovery": {"retry": null, "lease_recovered": false}, "state_version": 1}'::jsonb NOT NULL,
    step integer DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    error text,
    lease_owner uuid,
    lease_until timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    observed_input_seq bigint DEFAULT 0 NOT NULL,
    ledger_worker_ready boolean DEFAULT false NOT NULL,
    pending_human_request_id uuid GENERATED ALWAYS AS (
CASE
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'external_approval'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
    ELSE NULL::uuid
END) STORED,
    CONSTRAINT runs_control_check CHECK ((control = ANY (ARRAY['ACTIVE'::text, 'PAUSED'::text, 'CANCELLED'::text]))),
    CONSTRAINT runs_counters CHECK (((step >= 0) AND (revision >= 0) AND (revision < '9223372036854775807'::bigint))),
    CONSTRAINT runs_lease CHECK (COALESCE((((lease_owner IS NULL) = (lease_until IS NULL)) AND ((lease_until IS NULL) OR isfinite(lease_until))), false)),
    CONSTRAINT runs_phase_check CHECK ((phase = ANY (ARRAY['READY'::text, 'THINKING'::text, 'TOOL_CALL'::text, 'WAITING'::text, 'COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.run_activations ALTER COLUMN generation SET DEFAULT nextval('public.run_activations_generation_seq'::regclass);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.run_inputs ALTER COLUMN seq SET DEFAULT nextval('public.run_inputs_seq_seq'::regclass);"#.to_string(),
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
