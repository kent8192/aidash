ALTER TABLE public.remote_connection_calls ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.remote_connection_calls FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.remote_connection_calls AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
