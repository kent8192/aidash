ALTER TABLE public.core_areas ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_areas FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_areas AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_objects ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_objects FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_objects AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_operations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_quotas ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_quotas FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_quotas AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_records FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_records AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_requests AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_policies ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_policies FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_policies AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_policy_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_policy_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_policy_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_remote_intents ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_remote_intents FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_remote_intents AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_requests AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.research_revocations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_revocations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_revocations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
