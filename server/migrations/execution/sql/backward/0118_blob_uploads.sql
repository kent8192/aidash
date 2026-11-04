-- Stop at the chain head before a rollback can partially dismantle the
-- irreversible content-store cutover in 0113.
DO $$ BEGIN
  RAISE EXCEPTION 'execution 0113_content_object_references is irreversible; restore a pre-cutover backup'
    USING ERRCODE = '55000';
END $$;
DROP POLICY tenant_isolation ON public.core_blob_uploads;
ALTER TABLE public.core_blob_uploads NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_blob_uploads DISABLE ROW LEVEL SECURITY;
