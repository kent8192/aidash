// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0004_references", "registry")
        .add_dependency("marketplace", "0004_references")
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_draft_registrations
    ADD CONSTRAINT agent_draft_registrations_draft_id_fkey FOREIGN KEY (draft_id) REFERENCES public.agent_drafts(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_draft_shares
    ADD CONSTRAINT agent_draft_shares_draft_id_fkey FOREIGN KEY (draft_id) REFERENCES public.agent_drafts(id) ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_incident_events
    ADD CONSTRAINT agent_incident_events_incident_id_fkey FOREIGN KEY (incident_id) REFERENCES public.agent_incidents(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_knowledge
    ADD CONSTRAINT agent_knowledge_agent_id_agent_version_fkey FOREIGN KEY (agent_id, agent_version) REFERENCES public.registry(id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.agent_test_sessions
    ADD CONSTRAINT agent_test_sessions_draft_id_fkey FOREIGN KEY (draft_id) REFERENCES public.agent_drafts(id);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.installations
    ADD CONSTRAINT installations_id_version_fkey FOREIGN KEY (id, version) REFERENCES public.registry(id, version);"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry_agent_model_refs
    ADD CONSTRAINT registry_agent_model_source FOREIGN KEY (agent_id, agent_version) REFERENCES public.registry(id, version) ON UPDATE CASCADE ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry_agent_model_refs
    ADD CONSTRAINT registry_agent_model_target FOREIGN KEY (model_id, model_version, model_kind) REFERENCES public.registry(id, version, kind) ON UPDATE RESTRICT ON DELETE RESTRICT;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry_agent_resource_refs
    ADD CONSTRAINT registry_agent_resource_source FOREIGN KEY (agent_id, agent_version) REFERENCES public.registry(id, version) ON UPDATE CASCADE ON DELETE CASCADE;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"ALTER TABLE ONLY public.registry_agent_resource_refs
    ADD CONSTRAINT registry_agent_resource_target FOREIGN KEY (reference_id, reference_version, required_kind) REFERENCES public.registry(id, version, kind) ON UPDATE RESTRICT ON DELETE RESTRICT;"#.to_string(),
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
