ALTER TABLE public.approval_observer_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.approval_observer_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.approval_observer_settings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.approval_observations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.approval_observations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.approval_observations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.approval_observation_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.approval_observation_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.approval_observation_attempts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.approval_observation_population ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.approval_observation_population FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.approval_observation_population AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
