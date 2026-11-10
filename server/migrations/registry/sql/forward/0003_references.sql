SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE INDEX agent_drafts_tenant_owner ON public.agent_drafts USING btree (tenant, owner);

--

CREATE INDEX agent_incidents_version ON public.agent_incidents USING btree (agent_id, version);

--

CREATE INDEX agent_test_sessions_draft_created ON public.agent_test_sessions USING btree (draft_id, created_at);

--

CREATE INDEX registry_metadata ON public.registry USING gin (metadata);

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_draft_registrations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_draft_shares FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_drafts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_incident_events FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_incidents FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_knowledge FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_knowledge_images FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_test_limits FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_test_profiles FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_test_sessions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.registry FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.registry_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER marketplace_registry_fence BEFORE INSERT OR UPDATE ON public.registry FOR EACH ROW EXECUTE FUNCTION public.marketplace_registry_fence();

--

CREATE TRIGGER registry_agent_model_refs_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.registry_agent_model_refs FOR EACH STATEMENT EXECUTE FUNCTION public.guard_registry_agent_model_refs();

--

CREATE TRIGGER registry_agent_model_refs_sync AFTER INSERT OR UPDATE OF id, version, kind, metadata ON public.registry FOR EACH ROW EXECUTE FUNCTION public.sync_registry_agent_model_refs();

--

CREATE TRIGGER registry_agent_resource_refs_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.registry_agent_resource_refs FOR EACH STATEMENT EXECUTE FUNCTION public.guard_registry_agent_resource_refs();

--

CREATE TRIGGER registry_agent_resource_refs_sync AFTER INSERT OR UPDATE OF id, version, kind, metadata ON public.registry FOR EACH ROW EXECUTE FUNCTION public.sync_registry_agent_resource_refs();

--

CREATE TRIGGER registry_memory_role_refs_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.registry_memory_role_refs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER registry_memory_role_refs_guard BEFORE INSERT OR DELETE OR UPDATE ON public.registry_memory_role_refs FOR EACH STATEMENT EXECUTE FUNCTION public.guard_registry_memory_role_refs();

--

CREATE TRIGGER registry_memory_role_refs_sync AFTER INSERT OR UPDATE ON public.registry FOR EACH ROW EXECUTE FUNCTION public.sync_registry_memory_role_refs();

--

ALTER TABLE ONLY public.agent_draft_registrations
    ADD CONSTRAINT agent_draft_registrations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_draft_shares
    ADD CONSTRAINT agent_draft_shares_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_drafts
    ADD CONSTRAINT agent_drafts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_incident_events
    ADD CONSTRAINT agent_incident_events_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_incidents
    ADD CONSTRAINT agent_incidents_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_knowledge_images
    ADD CONSTRAINT agent_knowledge_images_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_knowledge
    ADD CONSTRAINT agent_knowledge_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_plugin_copies
    ADD CONSTRAINT agent_plugin_copies_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_test_limits
    ADD CONSTRAINT agent_test_limits_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_test_profiles
    ADD CONSTRAINT agent_test_profiles_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.agent_test_sessions
    ADD CONSTRAINT agent_test_sessions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.registry_agent_model_refs
    ADD CONSTRAINT registry_agent_model_refs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.registry_agent_resource_refs
    ADD CONSTRAINT registry_agent_resource_refs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.registry_memory_role_refs
    ADD CONSTRAINT registry_memory_role_refs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.registry_requests
    ADD CONSTRAINT registry_requests_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.registry
    ADD CONSTRAINT registry_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE public.agent_draft_registrations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_draft_shares ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_drafts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_incident_events ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_incidents ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_knowledge ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_knowledge_images ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_plugin_copies ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_test_limits ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_test_profiles ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.agent_test_sessions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.registry ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.registry_agent_model_refs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.registry_agent_resource_refs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.registry_memory_role_refs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.registry_requests ENABLE ROW LEVEL SECURITY;

--

CREATE POLICY tenant_isolation ON public.agent_draft_registrations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_draft_shares USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_drafts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_incident_events USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_incidents USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_knowledge USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_knowledge_images USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_plugin_copies USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_test_limits USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_test_profiles USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.agent_test_sessions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.registry USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.registry_agent_model_refs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.registry_agent_resource_refs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.registry_memory_role_refs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.registry_requests USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
