// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0003_keys_indexes", "workspaces")
        .add_dependency("registry", "0003_keys_indexes")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_idempotency_key_key UNIQUE (idempotency_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachment_idempotency UNIQUE (workspace_id, uploaded_by, idempotency_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachment_scope_key UNIQUE (workspace_id, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_message_context
    ADD CONSTRAINT channel_message_context_pkey PRIMARY KEY (message_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_thread_scope_key UNIQUE (workspace_id, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_root_message_id_key UNIQUE (root_message_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_idempotency_key_key UNIQUE (idempotency_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT task_dependencies_pkey PRIMARY KEY (task_id, dependency_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_completion_key_key UNIQUE (completion_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_creation_key_key UNIQUE (creation_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX channel_attachment_message_lookup ON public.channel_attachments USING btree (workspace_id, message_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX channel_message_history_order ON public.messages USING btree (workspace_id, created_at, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE UNIQUE INDEX channel_message_scope_key ON public.messages USING btree (workspace_id, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX channel_reply_lookup ON public.channel_message_context USING btree (workspace_id, thread_id, message_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX task_dependencies_target ON public.task_dependencies USING btree (dependency_id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE UNIQUE INDEX tasks_id_workspace_unique ON public.tasks USING btree (id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX tasks_workspace ON public.tasks USING btree (workspace_id, created_at);"#.to_string(),
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
