-- The Tenant root reference every Tenant-owned table carries (tenancy test).
-- Like 0077/0079, the physical key lives here and the logical model state
-- keeps no foreign key, so it is added in SQL rather than in CreateTable.
ALTER TABLE public.egress_usage_daily
    ADD CONSTRAINT egress_usage_daily_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE public.egress_usage_daily ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_usage_daily FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_usage_daily AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
