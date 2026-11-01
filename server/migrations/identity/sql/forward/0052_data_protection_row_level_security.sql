ALTER TABLE public.tenant_data_protection ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_data_protection FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.tenant_data_protection AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.tenant_data_protection_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_data_protection_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.tenant_data_protection_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
