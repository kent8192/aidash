SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE INDEX authorization_credentials_active_mapping ON public.authorization_credentials USING btree (mapping_id, expires_at) WHERE ((mapping_id IS NOT NULL) AND (revoked_at IS NULL));

--

CREATE INDEX authorization_credentials_mapping ON public.authorization_credentials USING btree (mapping_id, created_at, id) WHERE (mapping_id IS NOT NULL);

--

CREATE INDEX authorization_credentials_tenant ON public.authorization_credentials USING btree (tenant, created_at, id);

--

CREATE INDEX authorization_decisions_tenant ON public.authorization_decisions USING btree (tenant, sequence);

--

CREATE INDEX authorization_output_resource ON public.authorization_run_outputs USING btree (workspace_id, resource_kind, resource_id, run_id);

--

CREATE INDEX authorization_workspaces_tenant ON public.authorization_workspaces USING btree (tenant, workspace_id);

--

CREATE INDEX dashboard_login_browser ON public.dashboard_login_transactions USING btree (browser_hash);

--

CREATE INDEX dashboard_login_expiry ON public.dashboard_login_transactions USING btree (expires_at);

--

CREATE INDEX dashboard_logout_expiry ON public.dashboard_logout_tokens USING btree (expires_at);

--

CREATE INDEX dashboard_origin_identity_run ON public.dashboard_execution_origins USING btree (identity_id, run_id);

--

CREATE INDEX dashboard_registration_pending ON public.dashboard_registration_requests USING btree (status, expires_at);

--

CREATE INDEX dashboard_registration_tenant_pending ON public.dashboard_registration_requests USING btree (tenant, status, expires_at);

--

CREATE INDEX dashboard_session_expiry ON public.dashboard_sessions USING btree (expires_at);

--

CREATE INDEX dashboard_session_identity ON public.dashboard_sessions USING btree (identity_id);

--

CREATE INDEX dashboard_session_provider_sid ON public.dashboard_sessions USING btree (provider_sid);

--

CREATE INDEX desktop_handoff_expiry ON public.desktop_handoffs USING btree (expires_at);

--

CREATE INDEX desktop_refresh_session ON public.desktop_refresh_credentials USING btree (session_id);

--

CREATE INDEX idx_dashboard_mappings_tenant_subject_be6d4d1b7219390f ON public.dashboard_mappings USING btree (tenant, subject) WHERE enabled;

--

CREATE INDEX idx_run_plan_connections_tenant_subject_provid_f6d1d1b5176a0e4d ON public.run_plan_connections USING btree (tenant, subject, provider, plan_connection_id);

--

CREATE INDEX idx_tenant_retention_rule_history_tenant_changed_at ON public.tenant_retention_rule_history USING btree (tenant, changed_at);

--

CREATE INDEX node_deletions_audit_kept ON public.node_deletions USING btree (purged_at) WHERE ((purged_at IS NOT NULL) AND (audit_erased_at IS NULL));

--

CREATE INDEX node_deletions_awaiting_purge ON public.node_deletions USING btree (purge_after) WHERE ((restored_at IS NULL) AND (purged_at IS NULL));

--

CREATE INDEX tenant_deletions_audit_kept ON public.tenant_deletions USING btree (purged_at) WHERE ((purged_at IS NOT NULL) AND (audit_erased_at IS NULL));

--

CREATE INDEX tenant_deletions_awaiting_purge ON public.tenant_deletions USING btree (purge_after) WHERE ((restored_at IS NULL) AND (purged_at IS NULL));

--

CREATE INDEX tenant_invitations_active ON public.tenant_invitations USING btree (tenant, expires_at) WHERE ((revoked_at IS NULL) AND (redeemed_at IS NULL));

--

CREATE INDEX tenant_invitations_history ON public.tenant_invitations USING btree (tenant, created_at);

--

CREATE INDEX tenants_created ON public.tenants USING btree (created_at, name);

--

CREATE INDEX tenants_creator ON public.tenants USING btree (created_by_identity, created_at) WHERE (created_by_identity IS NOT NULL);

--

CREATE TRIGGER aidash_delegated_task_activation AFTER INSERT ON public.authorization_run_outputs FOR EACH ROW EXECUTE FUNCTION public.aidash_delegated_task_activation();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_bundles FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_catalog FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_catalog_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_credentials FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_decisions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_execution FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_commands FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_execution FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_outputs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_terminal_receipts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_revisions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_outputs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_registry_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_run_remote_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_task_origins FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_workspaces FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER marketplace_catalog_fence BEFORE INSERT OR UPDATE ON public.authorization_catalog FOR EACH ROW EXECUTE FUNCTION public.marketplace_catalog_fence();

--

CREATE TRIGGER node_deletions_immutable BEFORE UPDATE ON public.node_deletions FOR EACH ROW EXECUTE FUNCTION public.aidash_node_deletion_immutable();

--

ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_entry_id_entry_version_fkey FOREIGN KEY (tenant, entry_id, entry_version) REFERENCES public.registry(tenant, id, version);

--

ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.authorization_run_outputs
    ADD CONSTRAINT authorization_run_outputs_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.authorization_run_reads
    ADD CONSTRAINT authorization_run_reads_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.authorization_run_registry_reads
    ADD CONSTRAINT authorization_run_registry_reads_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.authorization_run_remote_reads
    ADD CONSTRAINT authorization_run_remote_reads_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_task_id_fkey FOREIGN KEY (tenant, task_id) REFERENCES public.tasks(tenant, id);

--

ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.remote_connection_calls
    ADD CONSTRAINT remote_connection_calls_grant_fkey FOREIGN KEY (tenant, grant_id) REFERENCES public.authorization_remote_grants(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE public.authorization_bundles ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_catalog ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_catalog_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_credentials ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_decisions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_execution ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_remote_commands ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_remote_execution ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_remote_outputs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_remote_terminal_receipts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_revisions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_run_outputs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_run_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_run_registry_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_run_remote_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_task_origins ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_workspaces ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.catalog_approval_requests ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.connection_scope_ceilings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.credential_store_resources ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.credential_store_versions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.dashboard_administration_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.dashboard_execution_origins ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.dashboard_mappings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.demo_reservations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.identity_approval_preferences ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.identity_approval_rules ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.identity_tenant_approval_settings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.identity_thread_approval_modes ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.node_deletions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.nodes ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.oauth_authorizations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.oauth_clients ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.oauth_connections ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.oauth_dynamic_clients ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.personal_tenants ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.plan_connections ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.provider_credential_bindings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.provider_credentials ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.remote_connection_calls ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.run_plan_connections ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.run_provider_credentials ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.tenant_data_protection ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.tenant_data_protection_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.tenant_invitations ENABLE ROW LEVEL SECURITY;

--

CREATE POLICY tenant_isolation ON public.authorization_bundles USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_catalog USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_catalog_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_credentials USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_decisions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_execution USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_remote_commands USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_remote_execution USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_remote_outputs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_remote_terminal_receipts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_revisions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_run_outputs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_run_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_run_registry_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_run_remote_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_task_origins USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_workspaces USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.catalog_approval_requests USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.connection_scope_ceilings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.credential_store_resources USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.credential_store_versions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.dashboard_administration_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.dashboard_execution_origins USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.dashboard_mappings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.demo_reservations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.identity_approval_preferences USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.identity_approval_rules USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.identity_tenant_approval_settings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.identity_thread_approval_modes USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.node_deletions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.nodes USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.oauth_authorizations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.oauth_clients USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.oauth_connections USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.oauth_dynamic_clients USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.personal_tenants USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.plan_connections USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.provider_credential_bindings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.provider_credentials USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.remote_connection_calls USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.run_plan_connections USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.run_provider_credentials USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.tenant_data_protection USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.tenant_data_protection_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.tenant_invitations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.tenant_retention_rule_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.tenant_retention_rules USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.tenant_secrets USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

ALTER TABLE public.tenant_retention_rule_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.tenant_retention_rules ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.tenant_secrets ENABLE ROW LEVEL SECURITY;
