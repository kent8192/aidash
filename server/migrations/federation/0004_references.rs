// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0004_references", "federation")
        .add_dependency("execution", "0004_references")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_gate
    ADD CONSTRAINT atomic_gate_transaction_id_fkey FOREIGN KEY (transaction_id) REFERENCES public.atomic_participants(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_peer_trust
    ADD CONSTRAINT atomic_peer_trust_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.peers(node_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_votes
    ADD CONSTRAINT atomic_votes_transaction_id_fkey FOREIGN KEY (transaction_id) REFERENCES public.atomic_coordinators(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_peer_mapping_history
    ADD CONSTRAINT authorization_peer_mapping_history_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.authorization_credentials(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_peer_mapping_history
    ADD CONSTRAINT authorization_peer_mapping_history_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.authorization_credentials(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_grant_reads
    ADD CONSTRAINT authorization_remote_grant_reads_grant_id_fkey FOREIGN KEY (grant_id) REFERENCES public.authorization_remote_grants(id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_grants
    ADD CONSTRAINT authorization_remote_grants_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.delegations
    ADD CONSTRAINT delegations_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.remote_run_message_fences
    ADD CONSTRAINT remote_run_message_fences_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id) ON DELETE CASCADE;"#.to_string(),
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
