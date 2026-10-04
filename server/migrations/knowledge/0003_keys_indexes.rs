// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0003_keys_indexes", "knowledge")
        .add_dependency("identity", "0003_keys_indexes")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_agent_memory
    ADD CONSTRAINT semantic_agent_memory_pkey PRIMARY KEY (entry_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_collections
    ADD CONSTRAINT semantic_collections_pkey PRIMARY KEY (collection);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_workspace_id_key_key UNIQUE (workspace_id, key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_history
    ADD CONSTRAINT semantic_history_pkey PRIMARY KEY (sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_collection_key UNIQUE (collection);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_pkey PRIMARY KEY (workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_points
    ADD CONSTRAINT semantic_points_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_remote_attempts
    ADD CONSTRAINT semantic_remote_attempts_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_remote_operations
    ADD CONSTRAINT semantic_remote_operations_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_remote_reads
    ADD CONSTRAINT semantic_remote_reads_pkey PRIMARY KEY (grant_id, admission_id, entry_id, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_remote_receipts
    ADD CONSTRAINT semantic_remote_receipts_pkey PRIMARY KEY (operation_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.semantic_run_reads
    ADD CONSTRAINT semantic_run_reads_pkey PRIMARY KEY (run_id, entry_id, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX semantic_remote_operation_run ON public.semantic_remote_operations USING btree (home_node, grant_id, admission_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX semantic_remote_receipts_run ON public.semantic_remote_receipts USING btree (run_id);"#.to_string(),
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
