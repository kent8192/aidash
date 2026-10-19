ALTER TABLE public.agent_draft_registrations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_draft_registrations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_draft_registrations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.agent_draft_shares ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_draft_shares FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_draft_shares AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.agent_incident_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_incident_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_incident_events AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.agent_knowledge ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_knowledge FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_knowledge AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.registry ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.registry FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.registry AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.registry_agent_model_refs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.registry_agent_model_refs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.registry_agent_model_refs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.registry_agent_resource_refs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.registry_agent_resource_refs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.registry_agent_resource_refs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.registry_memory_role_refs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.registry_memory_role_refs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.registry_memory_role_refs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.registry_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.registry_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.registry_requests AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
