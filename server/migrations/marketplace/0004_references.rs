// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0004_references", "marketplace")
        .add_dependency("knowledge", "0004_references")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_entry_id_entry_version_fkey FOREIGN KEY (entry_id, entry_version) REFERENCES public.registry(id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_installation_fkey FOREIGN KEY (installation) REFERENCES public.marketplace_installations(key);"#.to_string(),
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
