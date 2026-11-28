ALTER TABLE public.drive_run_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_run_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_run_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
