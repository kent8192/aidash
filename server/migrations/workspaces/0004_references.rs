// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0004_references", "workspaces")
        .add_dependency("registry", "0004_references")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_task_workspace FOREIGN KEY (task_id, workspace_id) REFERENCES public.tasks(id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_workspace_id_message_id_fkey FOREIGN KEY (workspace_id, message_id) REFERENCES public.messages(workspace_id, id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_message_context
    ADD CONSTRAINT channel_message_context_workspace_id_message_id_fkey FOREIGN KEY (workspace_id, message_id) REFERENCES public.messages(workspace_id, id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_message_context
    ADD CONSTRAINT channel_message_context_workspace_id_thread_id_fkey FOREIGN KEY (workspace_id, thread_id) REFERENCES public.channel_threads(workspace_id, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_workspace_id_root_message_id_fkey FOREIGN KEY (workspace_id, root_message_id) REFERENCES public.messages(workspace_id, id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT tasks_dependencies_source_workspace FOREIGN KEY (task_id, workspace_id) REFERENCES public.tasks(id, workspace_id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT tasks_dependencies_target_workspace FOREIGN KEY (dependency_id, workspace_id) REFERENCES public.tasks(id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.tasks(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_parent_workspace FOREIGN KEY (parent_id, workspace_id) REFERENCES public.tasks(id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
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
