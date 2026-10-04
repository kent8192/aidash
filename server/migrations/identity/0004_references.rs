// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0004_references", "identity")
        .add_dependency("federation", "0004_references")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_entry_id_entry_version_fkey FOREIGN KEY (entry_id, entry_version) REFERENCES public.registry(id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_catalog_history
    ADD CONSTRAINT authorization_catalog_history_tenant_entry_id_entry_versio_fkey FOREIGN KEY (tenant, entry_id, entry_version) REFERENCES public.authorization_catalog(tenant, entry_id, entry_version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_decisions
    ADD CONSTRAINT authorization_decisions_tenant_revision_fkey FOREIGN KEY (tenant, revision) REFERENCES public.authorization_revisions(tenant, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.authorization_credentials(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.authorization_workspaces(workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_graph_operator_grants
    ADD CONSTRAINT authorization_graph_operator_grants_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_revisions
    ADD CONSTRAINT authorization_revisions_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_outputs
    ADD CONSTRAINT authorization_run_outputs_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_reads
    ADD CONSTRAINT authorization_run_reads_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_reads
    ADD CONSTRAINT authorization_run_reads_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.authorization_workspaces(workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_registry_reads
    ADD CONSTRAINT authorization_run_registry_reads_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_remote_reads
    ADD CONSTRAINT authorization_run_remote_reads_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_source_run_id_fkey FOREIGN KEY (source_run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_mapping_id_fkey FOREIGN KEY (mapping_id) REFERENCES public.dashboard_mappings(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.authorization_credentials(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_operator_grants
    ADD CONSTRAINT dashboard_operator_grants_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_registration_requests
    ADD CONSTRAINT dashboard_registration_requests_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_sessions
    ADD CONSTRAINT dashboard_sessions_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);"#.to_string(),
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
