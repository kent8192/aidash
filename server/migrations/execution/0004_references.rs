// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0004_references", "execution")
        .add_dependency("workspaces", "0003_keys_indexes")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_runs_area_id_fkey FOREIGN KEY (area_id) REFERENCES public.core_areas(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_runs_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_budgets
    ADD CONSTRAINT generation_budgets_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.generation_requests(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_provider_id_provider_version_fkey FOREIGN KEY (provider_id, provider_version) REFERENCES public.registry(id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.generation_requests(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_provider_id_provider_version_fkey FOREIGN KEY (provider_id, provider_version) REFERENCES public.registry(id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.generation_requests(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_history
    ADD CONSTRAINT generation_history_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.generation_requests(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_policies
    ADD CONSTRAINT generation_policies_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_policy_history
    ADD CONSTRAINT generation_policy_history_tenant_policy_id_fkey FOREIGN KEY (tenant, policy_id) REFERENCES public.generation_policies(tenant, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_remote_intents
    ADD CONSTRAINT generation_remote_intents_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.authorization_credentials(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_remote_intents
    ADD CONSTRAINT generation_remote_intents_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.tasks(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.authorization_credentials(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_local_task_id_fkey FOREIGN KEY (local_task_id) REFERENCES public.tasks(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_local_workspace_id_fkey FOREIGN KEY (local_workspace_id) REFERENCES public.authorization_workspaces(workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_task_workspace FOREIGN KEY (local_task_id, local_workspace_id) REFERENCES public.tasks(id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_tenant_policy_id_policy_revision_fkey FOREIGN KEY (tenant, policy_id, policy_revision) REFERENCES public.generation_policy_history(tenant, policy_id, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_usage
    ADD CONSTRAINT generation_usage_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.generation_requests(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_usage
    ADD CONSTRAINT generation_usage_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_run_workspace FOREIGN KEY (run_id, workspace_id) REFERENCES public.runs(id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.invocations
    ADD CONSTRAINT invocations_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.run_inputs
    ADD CONSTRAINT run_inputs_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_human_request_ref FOREIGN KEY (pending_human_request_id, id) REFERENCES public.human_requests(id, run_id) ON UPDATE RESTRICT ON DELETE RESTRICT;"#.to_string(),
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
