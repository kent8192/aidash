// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0006_triggers", "marketplace")
        .add_dependency("knowledge", "0006_triggers")
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_audiences FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_consents FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_gate FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_installations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_provenance FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_revisions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_versions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER marketplace_immutable BEFORE DELETE OR UPDATE ON public.marketplace_revisions FOR EACH ROW EXECUTE FUNCTION public.marketplace_immutable();"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE TRIGGER marketplace_immutable BEFORE DELETE OR UPDATE ON public.marketplace_versions FOR EACH ROW EXECUTE FUNCTION public.marketplace_immutable();"#.to_string(),
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
