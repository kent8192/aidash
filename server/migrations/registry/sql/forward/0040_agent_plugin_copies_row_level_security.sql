ALTER TABLE public.agent_plugin_copies ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_plugin_copies FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_plugin_copies AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
