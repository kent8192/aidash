ALTER TABLE public.tenant_execution_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_execution_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.tenant_execution_settings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
