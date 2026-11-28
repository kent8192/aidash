-- DropTable's reversal can add this ORM-only check. The original physical
-- artifacts_kind_check was restored by the typed reversal.
ALTER TABLE public.artifacts DROP CONSTRAINT IF EXISTS artifacts_kind_model_enum_check;
ALTER TABLE public.artifacts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.artifacts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.artifacts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.artifacts
    FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
