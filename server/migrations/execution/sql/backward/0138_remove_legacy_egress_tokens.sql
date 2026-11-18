-- Restore the forced Tenant policy once the typed reversal has recreated both
-- tables.
ALTER TABLE public.egress_budgets ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_budgets FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_budgets AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.egress_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_sessions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
