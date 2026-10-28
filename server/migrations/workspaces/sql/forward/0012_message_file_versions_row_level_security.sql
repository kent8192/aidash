ALTER TABLE public.message_file_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.message_file_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.message_file_versions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
