ALTER TABLE public.marketplace_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.marketplace_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.marketplace_requests AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.marketplace_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.marketplace_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.marketplace_revisions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
