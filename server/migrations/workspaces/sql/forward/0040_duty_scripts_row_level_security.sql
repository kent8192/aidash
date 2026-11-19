ALTER TABLE public.duty_script_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.duty_script_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.duty_script_versions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.duty_script_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.duty_script_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.duty_script_runs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
