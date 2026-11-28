ALTER TABLE public.core_retention_holds ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_retention_holds FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_retention_holds AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_retention_files ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_retention_files FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_retention_files AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
CREATE OR REPLACE FUNCTION public.aidash_release_message_retention() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM public.core_retention_holds
     WHERE holder_kind = 'message'
       AND holder_id = OLD.id::text
       AND tenant = OLD.tenant;
    RETURN OLD;
END;
$$;
DROP TRIGGER IF EXISTS messages_release_retention ON public.messages;
CREATE TRIGGER messages_release_retention AFTER DELETE ON public.messages
    FOR EACH ROW EXECUTE FUNCTION public.aidash_release_message_retention();
