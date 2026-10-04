// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0006_triggers", "workspaces")
        .add_dependency("registry", "0006_triggers")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER aidash_activation AFTER UPDATE OF status ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.artifacts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.channel_attachments FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.channel_message_context FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.channel_threads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.conversations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.messages FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.tasks FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.workspaces FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER gate_legacy_federated_run_message BEFORE INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_federated_run_message();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER gate_legacy_run_message BEFORE INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_run_message();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER gate_legacy_run_output BEFORE INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_run_output();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER gate_remote_task_terminal BEFORE UPDATE OF status ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.gate_remote_task_terminal();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER legacy_run_input_bridge AFTER INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.legacy_run_input_bridge();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER task_dependencies_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.task_dependencies FOR EACH STATEMENT EXECUTE FUNCTION public.guard_task_dependencies();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER tasks_dependencies_sync AFTER INSERT OR UPDATE OF id, workspace_id, dependencies ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.sync_task_dependencies();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER tasks_hierarchy_serialize BEFORE INSERT OR UPDATE OF id, parent_id, workspace_id, dependencies ON public.tasks FOR EACH STATEMENT EXECUTE FUNCTION public.lock_task_hierarchy_before_change();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE CONSTRAINT TRIGGER tasks_parent_cycle_guard AFTER INSERT OR UPDATE OF id, parent_id, workspace_id ON public.tasks DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.guard_task_parent_cycle();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE CONSTRAINT TRIGGER zz_tasks_dependency_cycle_guard AFTER INSERT OR UPDATE OF id, parent_id, dependencies ON public.tasks DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.guard_task_dependency_cycle();"#.to_string(),
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
