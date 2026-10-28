ALTER TABLE public.run_safety_stops ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.run_safety_stops FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.run_safety_stops AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
