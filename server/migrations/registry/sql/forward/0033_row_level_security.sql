ALTER TABLE public.agent_drafts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_drafts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_drafts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.agent_incidents ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_incidents FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_incidents AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.agent_test_limits ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_test_limits FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_test_limits AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.agent_test_profiles ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_test_profiles FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_test_profiles AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.agent_test_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_test_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_test_sessions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
