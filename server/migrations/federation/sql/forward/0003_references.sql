SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE INDEX atomic_authority_pending ON public.atomic_authority_attempts USING btree (transaction_id) WHERE (outcome IS NULL);

--

CREATE INDEX atomic_authority_pending_peer ON public.atomic_authority_attempts USING btree (node_id, transaction_id) WHERE (outcome IS NULL);

--

CREATE INDEX authorization_peer_mapping_history_tenant_sequence ON public.authorization_peer_mapping_history USING btree (tenant, sequence);

--

CREATE INDEX delegations_retry ON public.delegations USING btree (next_attempt_at, created_at) WHERE (NOT delivered);

--

CREATE INDEX node_registration_history_tenant_sequence ON public.node_registration_history USING btree (tenant, sequence);

--

CREATE INDEX remote_run_message_fences_sequence ON public.remote_run_message_fences USING btree (task_id, run_id, input_seq);

--

CREATE TRIGGER atomic_immutable_decision BEFORE UPDATE ON public.atomic_coordinators FOR EACH ROW EXECUTE FUNCTION public.atomic_immutable_decision();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_peer_mapping_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_peer_mappings FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_admissions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_grant_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_grants FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.delegations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.node_registration_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.node_registrations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.remote_run_message_fences FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

ALTER TABLE ONLY public.authorization_peer_mapping_history
    ADD CONSTRAINT authorization_peer_mapping_history_credential_id_fkey FOREIGN KEY (tenant, credential_id) REFERENCES public.authorization_credentials(tenant, id);

--

ALTER TABLE ONLY public.authorization_peer_mapping_history
    ADD CONSTRAINT authorization_peer_mapping_history_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.authorization_peer_mapping_history
    ADD CONSTRAINT authorization_peer_mapping_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_credential_id_fkey FOREIGN KEY (tenant, credential_id) REFERENCES public.authorization_credentials(tenant, id);

--

ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name);

--

ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name);

--

ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_remote_grant_reads
    ADD CONSTRAINT authorization_remote_grant_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_remote_grants
    ADD CONSTRAINT authorization_remote_grants_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name);

--

ALTER TABLE ONLY public.authorization_remote_grants
    ADD CONSTRAINT authorization_remote_grants_task_id_fkey FOREIGN KEY (tenant, task_id) REFERENCES public.tasks(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.authorization_remote_grants
    ADD CONSTRAINT authorization_remote_grants_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.delegations
    ADD CONSTRAINT delegations_task_id_fkey FOREIGN KEY (tenant, task_id) REFERENCES public.tasks(tenant, id);

--

ALTER TABLE ONLY public.delegations
    ADD CONSTRAINT delegations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.node_registration_history
    ADD CONSTRAINT node_registration_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.node_registrations
    ADD CONSTRAINT node_registrations_node_fkey FOREIGN KEY (node, tenant) REFERENCES public.node_names(name, tenant);

--

ALTER TABLE ONLY public.node_registrations
    ADD CONSTRAINT node_registrations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.remote_run_message_fences
    ADD CONSTRAINT remote_run_message_fences_task_id_fkey FOREIGN KEY (tenant, task_id) REFERENCES public.tasks(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.remote_run_message_fences
    ADD CONSTRAINT remote_run_message_fences_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE public.authorization_peer_mapping_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_peer_mappings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_remote_admissions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_remote_grant_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_remote_grants ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.delegations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.node_registration_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.node_registrations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.remote_run_message_fences ENABLE ROW LEVEL SECURITY;

--

CREATE POLICY tenant_isolation ON public.authorization_peer_mapping_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_peer_mappings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_remote_admissions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_remote_grant_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.authorization_remote_grants USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.delegations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.node_registration_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.node_registrations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.remote_run_message_fences USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
