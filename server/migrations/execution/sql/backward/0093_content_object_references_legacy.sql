CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_objects FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_quotas FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
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
