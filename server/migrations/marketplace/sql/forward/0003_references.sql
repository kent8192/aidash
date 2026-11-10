SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE INDEX marketplace_audiences_tenants ON public.marketplace_audiences USING gin (((document -> 'tenants'::text)));

--

CREATE INDEX marketplace_versions_source_content ON public.marketplace_versions USING btree (owner, source_id, source_version, source_content);

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_audiences FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_consents FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_installations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_provenance FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_revisions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_versions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER marketplace_immutable BEFORE DELETE OR UPDATE ON public.marketplace_revisions FOR EACH ROW EXECUTE FUNCTION public.marketplace_immutable();

--

CREATE TRIGGER marketplace_immutable BEFORE DELETE OR UPDATE ON public.marketplace_versions FOR EACH ROW EXECUTE FUNCTION public.marketplace_immutable();

--

ALTER TABLE ONLY public.marketplace_installations
    ADD CONSTRAINT marketplace_installations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.marketplace_requests
    ADD CONSTRAINT marketplace_requests_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_entry_id_entry_version_fkey FOREIGN KEY (tenant, entry_id, entry_version) REFERENCES public.registry(tenant, id, version);

--

ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE public.marketplace_installations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.marketplace_requests ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.marketplace_revisions ENABLE ROW LEVEL SECURITY;

--

CREATE POLICY tenant_isolation ON public.marketplace_installations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.marketplace_requests USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.marketplace_revisions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
