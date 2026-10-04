// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0003_keys_indexes", "marketplace")
        .add_dependency("knowledge", "0003_keys_indexes")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_audiences
    ADD CONSTRAINT marketplace_audiences_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_consents
    ADD CONSTRAINT marketplace_consents_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_gate
    ADD CONSTRAINT marketplace_gate_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_installations
    ADD CONSTRAINT marketplace_installations_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_installations
    ADD CONSTRAINT marketplace_installations_tenant_package_key_key UNIQUE (tenant, package_key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_provenance
    ADD CONSTRAINT marketplace_provenance_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_requests
    ADD CONSTRAINT marketplace_requests_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_entry_id_entry_version_key UNIQUE (entry_id, entry_version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_installation_revision_key UNIQUE (installation, revision);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_versions
    ADD CONSTRAINT marketplace_versions_pkey PRIMARY KEY (key);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.marketplace_versions
    ADD CONSTRAINT marketplace_versions_repository_owner_package_id_version_key UNIQUE (repository, owner, package_id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX marketplace_audiences_tenants ON public.marketplace_audiences USING gin (((document -> 'tenants'::text)));"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE INDEX marketplace_versions_source_content ON public.marketplace_versions USING btree (owner, source_id, source_version, source_content);"#.to_string(),
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
