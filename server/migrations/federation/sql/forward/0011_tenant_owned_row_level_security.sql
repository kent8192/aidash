ALTER TABLE public.authorization_remote_grant_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_grant_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_remote_grant_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.delegations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.delegations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.delegations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.remote_run_message_fences ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.remote_run_message_fences FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.remote_run_message_fences AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
