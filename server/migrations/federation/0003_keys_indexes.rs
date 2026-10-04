// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0003_keys_indexes", "federation")
        .add_dependency("execution", "0003_keys_indexes")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_authority_attempts
    ADD CONSTRAINT atomic_authority_attempts_pkey PRIMARY KEY (transaction_id, node_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_coordinators
    ADD CONSTRAINT atomic_coordinators_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_gate
    ADD CONSTRAINT atomic_gate_pkey PRIMARY KEY (singleton);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_history
    ADD CONSTRAINT atomic_history_pkey PRIMARY KEY (sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_participants
    ADD CONSTRAINT atomic_participants_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_peer_trust
    ADD CONSTRAINT atomic_peer_trust_pkey PRIMARY KEY (node_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_preflights
    ADD CONSTRAINT atomic_preflights_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_subjects
    ADD CONSTRAINT atomic_subjects_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.atomic_votes
    ADD CONSTRAINT atomic_votes_pkey PRIMARY KEY (transaction_id, node_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_peer_mapping_history
    ADD CONSTRAINT authorization_peer_mapping_history_pkey PRIMARY KEY (sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_pkey PRIMARY KEY (source_node, source_tenant, source_subject);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_source_node_grant_id_key UNIQUE (source_node, grant_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_source_node_task_id_key UNIQUE (source_node, task_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_grant_reads
    ADD CONSTRAINT authorization_remote_grant_reads_pkey PRIMARY KEY (grant_id, workspace_id, resource_kind, resource_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.authorization_remote_grants
    ADD CONSTRAINT authorization_remote_grants_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.delegations
    ADD CONSTRAINT delegations_pkey PRIMARY KEY (task_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.peer_events
    ADD CONSTRAINT peer_events_pkey PRIMARY KEY (node_id, event_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.peers
    ADD CONSTRAINT peers_pkey PRIMARY KEY (node_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.remote_run_message_fences
    ADD CONSTRAINT remote_run_message_fences_pkey PRIMARY KEY (task_id, idempotency_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX atomic_authority_pending ON public.atomic_authority_attempts USING btree (transaction_id) WHERE (outcome IS NULL);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX atomic_authority_pending_peer ON public.atomic_authority_attempts USING btree (node_id, transaction_id) WHERE (outcome IS NULL);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX authorization_peer_mapping_history_tenant_sequence ON public.authorization_peer_mapping_history USING btree (tenant, sequence);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX delegations_retry ON public.delegations USING btree (next_attempt_at, created_at) WHERE (NOT delivered);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX remote_run_message_fences_sequence ON public.remote_run_message_fences USING btree (task_id, run_id, input_seq);"#.to_string(),
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
