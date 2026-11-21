ALTER TABLE public.budget_lineage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.budget_lineage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.budget_lineage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
