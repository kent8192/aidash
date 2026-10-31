ALTER TABLE public.subnodes ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.subnodes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.subnodes AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.subnode_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.subnode_members FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.subnode_members AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.subnode_operation_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.subnode_operation_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.subnode_operation_receipts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.subnode_limits ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.subnode_limits FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.subnode_limits AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
