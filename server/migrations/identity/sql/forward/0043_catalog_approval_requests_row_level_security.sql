ALTER TABLE public.catalog_approval_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.catalog_approval_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.catalog_approval_requests AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
