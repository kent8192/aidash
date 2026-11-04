DROP POLICY tenant_isolation ON public.core_blob_uploads;
ALTER TABLE public.core_blob_uploads NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_blob_uploads DISABLE ROW LEVEL SECURITY;
