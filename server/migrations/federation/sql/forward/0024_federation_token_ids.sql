ALTER TABLE public.federation_token_ids ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.federation_token_ids FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.federation_token_ids AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
