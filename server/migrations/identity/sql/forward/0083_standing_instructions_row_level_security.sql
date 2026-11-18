ALTER TABLE public.tenant_instructions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_instructions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.tenant_instructions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.node_instructions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.node_instructions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.node_instructions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
