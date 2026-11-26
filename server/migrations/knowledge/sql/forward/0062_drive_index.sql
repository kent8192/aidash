ALTER TABLE public.drive_index_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_index_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_index_requests AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_passages ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_passages FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_passages AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
