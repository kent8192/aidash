ALTER TABLE public.semantic_image_ledgers ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_image_ledgers FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_image_ledgers AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_image_publications ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_image_publications FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_image_publications AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
