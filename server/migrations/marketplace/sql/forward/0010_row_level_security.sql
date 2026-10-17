ALTER TABLE public.marketplace_installations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.marketplace_installations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.marketplace_installations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
