// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0003_keys_indexes", "execution")
        .add_dependency("workspaces", "0002_tables")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.activation_quarantine
    ADD CONSTRAINT activation_quarantine_pkey PRIMARY KEY (digest);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_areas
    ADD CONSTRAINT core_areas_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_objects
    ADD CONSTRAINT core_objects_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_operations
    ADD CONSTRAINT core_operation_key UNIQUE (tenant, principal, request_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_operations
    ADD CONSTRAINT core_operations_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_quotas
    ADD CONSTRAINT core_quotas_pkey PRIMARY KEY (tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_records
    ADD CONSTRAINT core_records_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_requests
    ADD CONSTRAINT core_requests_pkey PRIMARY KEY (tenant, principal, key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_run_order UNIQUE (area_id, sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_runs_pkey PRIMARY KEY (run_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_areas
    ADD CONSTRAINT core_session_identity UNIQUE (tenant, home_node, workspace_id, thread_id, agent_id, owner);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.core_task_sessions
    ADD CONSTRAINT core_task_sessions_pkey PRIMARY KEY (task_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_id_key UNIQUE (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_pkey PRIMARY KEY (sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_budgets
    ADD CONSTRAINT generation_budgets_pkey PRIMARY KEY (request_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_pkey PRIMARY KEY (request_id, attempt_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_pkey PRIMARY KEY (request_id, attempt_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_history
    ADD CONSTRAINT generation_history_pkey PRIMARY KEY (sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_policies
    ADD CONSTRAINT generation_policies_pkey PRIMARY KEY (tenant, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_policy_history
    ADD CONSTRAINT generation_policy_history_pkey PRIMARY KEY (tenant, policy_id, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_remote_dispatches
    ADD CONSTRAINT generation_remote_dispatches_pkey PRIMARY KEY (attempt_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_remote_finalizations
    ADD CONSTRAINT generation_remote_finalizations_pkey PRIMARY KEY (attempt_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_remote_intents
    ADD CONSTRAINT generation_remote_intents_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_remote_usage
    ADD CONSTRAINT generation_remote_usage_pkey PRIMARY KEY (request_id, attempt_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_agent_id_key UNIQUE (agent_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.generation_usage
    ADD CONSTRAINT generation_usage_pkey PRIMARY KEY (request_id, attempt_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_id_run_id_key UNIQUE (id, run_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_request_key_key UNIQUE (request_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.inbox
    ADD CONSTRAINT inbox_pkey PRIMARY KEY (event_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.invocations
    ADD CONSTRAINT invocations_pkey PRIMARY KEY (idempotency_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.memory
    ADD CONSTRAINT memory_next_pkey PRIMARY KEY (agent_id, agent_version, workspace_id, home_node);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.run_activations
    ADD CONSTRAINT run_activations_id_key UNIQUE (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.run_activations
    ADD CONSTRAINT run_activations_pkey PRIMARY KEY (generation);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.run_inputs
    ADD CONSTRAINT run_inputs_pkey PRIMARY KEY (seq);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.run_inputs
    ADD CONSTRAINT run_inputs_run_key UNIQUE (run_id, idempotency_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_home_node_task_id_key UNIQUE (home_node, task_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX activation_dependency_runs ON public.runs USING btree (task_id) WHERE (phase <> ALL (ARRAY['COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text]));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX activation_due ON public.run_activations USING btree (due_at, generation) WHERE (state <> 'settled'::text);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX activation_run_generation ON public.run_activations USING btree (run_id, generation);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX core_operation_pending_queue ON public.core_operations USING btree (updated_at) WHERE (state = ANY (ARRAY['prepared'::text, 'submitted'::text, 'running'::text, 'cancelling'::text]));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX dashboard_status_waiting_runs ON public.runs USING btree (id) WHERE ((control = 'PAUSED'::text) AND (error = 'identity status unavailable'::text));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX events_marketplace_package_sequence ON public.events USING btree (((data ->> 'key'::text)), sequence) WHERE ((workspace_id IS NULL) AND (kind ~~ 'marketplace.%'::text) AND (kind <> 'marketplace.audit'::text));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX events_marketplace_tenant_sequence ON public.events USING btree (((data ->> 'tenant'::text)), sequence) WHERE ((workspace_id IS NULL) AND (kind ~~ 'marketplace.%'::text) AND (kind <> 'marketplace.audit'::text));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX events_outbox ON public.events USING btree (sequence) WHERE (published_at IS NULL);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX events_retry ON public.events USING btree (next_attempt_at, sequence) WHERE (published_at IS NULL);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX events_workspace ON public.events USING btree (workspace_id, sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX generation_remote_dispatches_pending_finalization ON public.generation_remote_dispatches USING btree (created_at) WHERE ((state = ANY (ARRAY['ABORTED'::text, 'SETTLED'::text])) AND (peer_finalized = false));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX generation_remote_dispatches_preparing ON public.generation_remote_dispatches USING btree (created_at) WHERE (state = 'PREPARING'::text);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX generation_remote_intents_cancel_retry ON public.generation_remote_intents USING btree (cancelled, cancel_delivered, cancel_retry_at);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX generation_remote_usage_attempt_digest ON public.generation_remote_usage USING btree (attempt_id, digest);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE UNIQUE INDEX generation_requests_home_task ON public.generation_requests USING btree (home_node, task_id) WHERE ((home_node <> ''::text) AND (status = ANY (ARRAY['PENDING_APPROVAL'::text, 'QUEUED'::text, 'ACTIVE'::text])));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE UNIQUE INDEX generation_requests_local_task ON public.generation_requests USING btree (task_id) WHERE (home_node = ''::text);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX generation_requests_policy ON public.generation_requests USING btree (tenant, policy_id, status);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE UNIQUE INDEX runs_id_workspace_unique ON public.runs USING btree (id, workspace_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX runs_ready ON public.runs USING btree (updated_at) WHERE (phase <> ALL (ARRAY['COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text]));"#.to_string(),
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
