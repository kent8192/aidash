ALTER TABLE public.egress_run_revocations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_run_revocations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_run_revocations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
