ALTER TABLE public.authorization_remote_terminal_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_terminal_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_remote_terminal_receipts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_terminal_receipts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
