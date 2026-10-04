// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0002_tables", "workspaces")
        .add_dependency("registry", "0002_tables")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.artifacts (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    kind text NOT NULL,
    name text NOT NULL,
    content jsonb NOT NULL,
    created_by text NOT NULL,
    idempotency_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT artifacts_kind_check CHECK ((kind = ANY (ARRAY['text'::text, 'json'::text, 'file_reference'::text, 'code'::text, 'structured_result'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.channel_attachments (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    uploaded_by text NOT NULL,
    idempotency_key uuid NOT NULL,
    filename text NOT NULL,
    media_type text NOT NULL,
    sha256 text NOT NULL,
    size_bytes bigint NOT NULL,
    content bytea NOT NULL,
    message_id uuid,
    "position" integer DEFAULT 0 NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.channel_message_context (
    message_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    thread_id uuid,
    attachment_digest text DEFAULT 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'::text NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.channel_threads (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    root_message_id uuid NOT NULL,
    created_by text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.conversations (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    target text NOT NULL,
    target_kind text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    created_by text DEFAULT 'human'::text NOT NULL,
    CONSTRAINT conversations_target_kind_check CHECK ((target_kind = ANY (ARRAY['agent'::text, 'cluster'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.messages (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    sender text NOT NULL,
    content text NOT NULL,
    idempotency_key text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.task_dependencies (
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    dependency_id uuid NOT NULL
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.tasks (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    title text NOT NULL,
    description text NOT NULL,
    status text DEFAULT 'OPEN'::text NOT NULL,
    requirements jsonb DEFAULT '{}'::jsonb NOT NULL,
    owner text,
    created_by text NOT NULL,
    dependencies uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    parent_id uuid,
    revision bigint DEFAULT 0 NOT NULL,
    creation_key text,
    completion_key text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT tasks_active_owner CHECK (COALESCE(((status <> ALL (ARRAY['CLAIMED'::text, 'RUNNING'::text])) OR (owner IS NOT NULL)), false)),
    CONSTRAINT tasks_check CHECK ((((status = 'OPEN'::text) AND (owner IS NULL)) OR (status <> 'OPEN'::text))),
    CONSTRAINT tasks_content CHECK (COALESCE(((length(btrim(title, '	

                  　'::text)) > 0) AND (length(btrim(description, '	

                  　'::text)) > 0) AND (jsonb_typeof(requirements) = 'object'::text) AND (revision >= 0) AND (revision < '9223372036854775807'::bigint) AND
CASE
    WHEN (jsonb_typeof(requirements) = 'object'::text) THEN ((requirements - ARRAY['kind'::text, 'query'::text, 'capability'::text, 'language'::text, 'skill'::text, 'tag'::text, 'model'::text]) = '{}'::jsonb)
    ELSE false
END AND (NOT jsonb_path_exists(requirements, 'strict $.*?(@.type() != "string" && @.type() != "null")'::jsonpath, '{}'::jsonb, true))), false)),
    CONSTRAINT tasks_no_self_reference CHECK (COALESCE((((parent_id IS NULL) OR (parent_id <> id)) AND (NOT (id = ANY (dependencies))) AND (array_position(dependencies, NULL::uuid) IS NULL)), false)),
    CONSTRAINT tasks_status_check CHECK ((status = ANY (ARRAY['OPEN'::text, 'CLAIMED'::text, 'RUNNING'::text, 'COMPLETED'::text, 'FAILED'::text, 'BLOCKED'::text, 'CANCELLED'::text, 'ABANDONED'::text])))
);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TABLE public.workspaces (
    id uuid NOT NULL,
    title text NOT NULL,
    goal text NOT NULL,
    state jsonb DEFAULT '{}'::jsonb NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT workspaces_content CHECK (COALESCE(((length(btrim(title, '	

                  　'::text)) > 0) AND (length(btrim(goal, '	

                  　'::text)) > 0) AND (jsonb_typeof(state) = 'object'::text) AND (revision >= 0)), false))
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
