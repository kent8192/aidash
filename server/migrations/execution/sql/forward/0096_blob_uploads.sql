-- Reinhardt does not yet express PostgreSQL row-level security policies.
ALTER TABLE public.core_blob_uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_blob_uploads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_blob_uploads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
