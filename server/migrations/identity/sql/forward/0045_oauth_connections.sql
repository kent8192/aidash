ALTER TABLE public.oauth_connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_connections FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.oauth_connections AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.oauth_authorizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_authorizations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.oauth_authorizations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.oauth_dynamic_clients ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_dynamic_clients FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.oauth_dynamic_clients AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
