ALTER TABLE public.scratch_spaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.scratch_spaces FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.scratch_spaces AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.scratch_space_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.scratch_space_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.scratch_space_settings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.scratch_space_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.scratch_space_members FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.scratch_space_members AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
