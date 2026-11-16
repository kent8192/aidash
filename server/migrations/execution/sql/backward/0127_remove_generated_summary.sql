CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_summary_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
ALTER TABLE public.generation_summary_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_summary_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_summary_usage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
