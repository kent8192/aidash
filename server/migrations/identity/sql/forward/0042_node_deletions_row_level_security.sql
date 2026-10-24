ALTER TABLE public.node_deletions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.node_deletions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.node_deletions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
