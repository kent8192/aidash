ALTER TABLE public.authorization_remote_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_commands FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_remote_commands AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_remote_execution ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_execution FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_remote_execution AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_remote_outputs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_outputs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_remote_outputs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_run_outputs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_run_outputs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_run_outputs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_run_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_run_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_run_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_run_registry_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_run_registry_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_run_registry_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_run_remote_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_run_remote_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_run_remote_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.dashboard_execution_origins ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.dashboard_execution_origins FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.dashboard_execution_origins AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
