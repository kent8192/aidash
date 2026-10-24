ALTER TABLE public.node_cancellation_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.node_cancellation_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.node_cancellation_jobs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
