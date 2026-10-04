// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0003_keys_indexes", "registry")
        .add_dependency("marketplace", "0003_keys_indexes")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_draft_registrations
    ADD CONSTRAINT agent_draft_registrations_pkey PRIMARY KEY (draft_id, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_draft_shares
    ADD CONSTRAINT agent_draft_shares_pkey PRIMARY KEY (draft_id, subject);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_drafts
    ADD CONSTRAINT agent_drafts_managed_id_key UNIQUE (managed_id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_drafts
    ADD CONSTRAINT agent_drafts_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_incident_events
    ADD CONSTRAINT agent_incident_events_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_incidents
    ADD CONSTRAINT agent_incidents_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_knowledge
    ADD CONSTRAINT agent_knowledge_pkey PRIMARY KEY (agent_id, agent_version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_test_limits
    ADD CONSTRAINT agent_test_limits_pkey PRIMARY KEY (tenant);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_test_profiles
    ADD CONSTRAINT agent_test_profiles_pkey PRIMARY KEY (tenant, id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_test_sessions
    ADD CONSTRAINT agent_test_sessions_active_slot_key UNIQUE (active_slot);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_test_sessions
    ADD CONSTRAINT agent_test_sessions_pkey PRIMARY KEY (id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.installations
    ADD CONSTRAINT installations_pkey PRIMARY KEY (id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.packages
    ADD CONSTRAINT packages_pkey PRIMARY KEY (id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry_agent_model_refs
    ADD CONSTRAINT registry_agent_model_refs_pkey PRIMARY KEY (agent_id, agent_version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry_agent_resource_refs
    ADD CONSTRAINT registry_agent_resource_refs_pkey PRIMARY KEY (agent_id, agent_version, required_kind, ordinal);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry
    ADD CONSTRAINT registry_pkey PRIMARY KEY (id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry_requests
    ADD CONSTRAINT registry_requests_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE UNIQUE INDEX agent_draft_registered_version ON public.agent_draft_registrations USING btree (agent_id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX agent_drafts_tenant_owner ON public.agent_drafts USING btree (tenant, owner);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX agent_incidents_version ON public.agent_incidents USING btree (agent_id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX agent_test_sessions_draft_created ON public.agent_test_sessions USING btree (draft_id, created_at);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE UNIQUE INDEX registry_id_version_kind_unique ON public.registry USING btree (id, version, kind);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX registry_metadata ON public.registry USING gin (metadata);"#.to_string(),
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
