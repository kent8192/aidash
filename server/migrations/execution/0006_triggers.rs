// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0006_triggers", "execution")
        .add_dependency("marketplace", "0005_seed")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER aidash_activation AFTER UPDATE OF state ON public.core_areas FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE OF state, expires_at ON public.core_records FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE OF initialized ON public.core_runs FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER aidash_activation AFTER UPDATE OF response ON public.human_requests FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER aidash_activation AFTER INSERT ON public.run_inputs FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE ON public.runs FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_areas FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_objects FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_operations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_quotas FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_records FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_runs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_task_sessions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.events FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_budgets FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_compaction_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_embedding_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_policies FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_policy_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_dispatches FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_finalizations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_intents FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.human_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.inbox FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.invocations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.run_inputs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.runs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER gate_legacy_run_worker BEFORE UPDATE ON public.runs FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_run_worker();"#.to_string(),
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
