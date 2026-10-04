// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0004_references", "knowledge")
        .add_dependency("identity", "0004_references")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_agent_memory
    ADD CONSTRAINT semantic_agent_memory_entry_id_fkey FOREIGN KEY (entry_id) REFERENCES public.semantic_entries(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_collections
    ADD CONSTRAINT semantic_collections_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.semantic_indexes(workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.semantic_indexes(workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_points
    ADD CONSTRAINT semantic_points_collection_fkey FOREIGN KEY (collection) REFERENCES public.semantic_collections(collection);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_points
    ADD CONSTRAINT semantic_points_entry_id_fkey FOREIGN KEY (entry_id) REFERENCES public.semantic_entries(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_run_reads
    ADD CONSTRAINT semantic_run_reads_entry_id_fkey FOREIGN KEY (entry_id) REFERENCES public.semantic_entries(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_run_reads
    ADD CONSTRAINT semantic_run_reads_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
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
