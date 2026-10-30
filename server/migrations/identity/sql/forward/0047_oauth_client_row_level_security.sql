ALTER TABLE public.oauth_clients ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_clients FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.oauth_clients AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.connection_scope_ceilings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.connection_scope_ceilings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.connection_scope_ceilings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
