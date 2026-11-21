ALTER TABLE public.model_calls ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.model_calls FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.model_calls AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
