ALTER TABLE public.core_objects ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_objects FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_objects AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_quotas ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_quotas FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_quotas AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_retention_files ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_retention_files FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_retention_files AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
