ALTER TABLE public.core_blobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_blobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_blobs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_manifest_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_manifest_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_manifest_revisions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_blob_references ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_blob_references FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_blob_references AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
