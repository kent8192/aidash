ALTER TABLE public.egress_usage_daily ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_usage_daily FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_usage_daily AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
