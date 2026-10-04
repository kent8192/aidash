// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0006_triggers", "identity")
        .add_dependency("federation", "0006_triggers")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_bundles FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_catalog FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_catalog_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_credentials FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_decisions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_execution FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_graph_operator_grants FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_commands FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_execution FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_outputs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_revisions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_outputs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_registry_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_remote_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_task_origins FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_workspaces FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER marketplace_catalog_fence BEFORE INSERT OR UPDATE ON public.authorization_catalog FOR EACH ROW EXECUTE FUNCTION public.marketplace_catalog_fence();"#.to_string(),
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
