ALTER TABLE public.budgets ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.budgets FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.budgets AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.budget_notices ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.budget_notices FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.budget_notices AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.budget_spend ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.budget_spend FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.budget_spend AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
