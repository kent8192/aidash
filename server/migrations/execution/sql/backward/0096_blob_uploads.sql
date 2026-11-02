DROP POLICY IF EXISTS tenant_isolation ON public.core_blob_uploads;
ALTER TABLE public.core_blob_uploads DISABLE ROW LEVEL SECURITY;
