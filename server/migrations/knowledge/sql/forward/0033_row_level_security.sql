ALTER TABLE public.memory_banks ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_banks FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_banks AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_participants ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_participants FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_participants AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_indexes ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_indexes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_indexes AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_vectors ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_vectors FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_vectors AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
