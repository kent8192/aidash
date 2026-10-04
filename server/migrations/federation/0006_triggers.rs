// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0006_triggers", "federation")
        .add_dependency("execution", "0006_triggers")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_immutable_decision BEFORE UPDATE ON public.atomic_coordinators FOR EACH ROW EXECUTE FUNCTION public.atomic_immutable_decision();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_peer_mapping_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_peer_mappings FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_admissions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_grant_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_grants FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.delegations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.peer_events FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.peers FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.remote_run_message_fences FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
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
