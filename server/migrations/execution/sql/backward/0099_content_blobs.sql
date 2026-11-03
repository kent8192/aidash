DROP POLICY tenant_isolation ON public.core_blob_references;
ALTER TABLE public.core_blob_references NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_blob_references DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_manifest_revisions;
ALTER TABLE public.core_manifest_revisions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_manifest_revisions DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_blobs;
ALTER TABLE public.core_blobs NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_blobs DISABLE ROW LEVEL SECURITY;
