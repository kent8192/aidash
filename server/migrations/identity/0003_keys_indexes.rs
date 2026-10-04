// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0003_keys_indexes", "identity")
        .add_dependency("federation", "0003_keys_indexes")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_bundles
    ADD CONSTRAINT authorization_bundles_pkey PRIMARY KEY (tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_catalog_history
    ADD CONSTRAINT authorization_catalog_history_pkey PRIMARY KEY (tenant, entry_id, entry_version, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_pkey PRIMARY KEY (tenant, entry_id, entry_version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_token_hash_key UNIQUE (token_hash);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_decisions
    ADD CONSTRAINT authorization_decisions_pkey PRIMARY KEY (sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_pkey PRIMARY KEY (run_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_task_id_key UNIQUE (task_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_graph_operator_grants
    ADD CONSTRAINT authorization_graph_operator_grants_pkey PRIMARY KEY (source_node, source_operator, tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_commands
    ADD CONSTRAINT authorization_remote_commands_pkey PRIMARY KEY (grant_id, request_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_execution
    ADD CONSTRAINT authorization_remote_execution_admission_id_key UNIQUE (admission_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_execution
    ADD CONSTRAINT authorization_remote_execution_pkey PRIMARY KEY (grant_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_execution
    ADD CONSTRAINT authorization_remote_execution_task_id_key UNIQUE (task_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_outputs
    ADD CONSTRAINT authorization_remote_outputs_pkey PRIMARY KEY (grant_id, workspace_id, resource_kind, resource_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_revisions
    ADD CONSTRAINT authorization_revisions_pkey PRIMARY KEY (tenant, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_outputs
    ADD CONSTRAINT authorization_run_outputs_pkey PRIMARY KEY (run_id, resource_kind, resource_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_reads
    ADD CONSTRAINT authorization_run_reads_pkey PRIMARY KEY (run_id, resource_kind, resource_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_registry_reads
    ADD CONSTRAINT authorization_run_registry_reads_pkey PRIMARY KEY (run_id, entry_id, entry_version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_run_remote_reads
    ADD CONSTRAINT authorization_run_remote_reads_pkey PRIMARY KEY (run_id, node_id, entry_id, entry_version, digest);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_pkey PRIMARY KEY (task_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_pkey PRIMARY KEY (workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_pkey PRIMARY KEY (run_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_identities
    ADD CONSTRAINT dashboard_identities_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_identities
    ADD CONSTRAINT dashboard_identity_key UNIQUE (issuer, subject);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_login_transactions
    ADD CONSTRAINT dashboard_login_transactions_pkey PRIMARY KEY (state_hash);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_logout_tokens
    ADD CONSTRAINT dashboard_logout_tokens_pkey PRIMARY KEY (jti_hash);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mapping_key UNIQUE (identity_id, tenant, subject);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_credential_id_key UNIQUE (credential_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_operator_grants
    ADD CONSTRAINT dashboard_operator_grants_pkey PRIMARY KEY (identity_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_registration_requests
    ADD CONSTRAINT dashboard_registration_requests_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_sessions
    ADD CONSTRAINT dashboard_sessions_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.dashboard_sessions
    ADD CONSTRAINT dashboard_sessions_token_hash_key UNIQUE (token_hash);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX authorization_credentials_tenant ON public.authorization_credentials USING btree (tenant, created_at, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX authorization_decisions_tenant ON public.authorization_decisions USING btree (tenant, sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX authorization_output_resource ON public.authorization_run_outputs USING btree (workspace_id, resource_kind, resource_id, run_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX authorization_workspaces_tenant ON public.authorization_workspaces USING btree (tenant, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_login_browser ON public.dashboard_login_transactions USING btree (browser_hash);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_login_expiry ON public.dashboard_login_transactions USING btree (expires_at);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_logout_expiry ON public.dashboard_logout_tokens USING btree (expires_at);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_origin_identity_run ON public.dashboard_execution_origins USING btree (identity_id, run_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_registration_pending ON public.dashboard_registration_requests USING btree (status, expires_at);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_session_expiry ON public.dashboard_sessions USING btree (expires_at);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_session_identity ON public.dashboard_sessions USING btree (identity_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_session_provider_sid ON public.dashboard_sessions USING btree (provider_sid);"#.to_string(),
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
