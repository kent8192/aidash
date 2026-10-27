DROP POLICY tenant_isolation ON public.core_retention_files;
ALTER TABLE public.core_retention_files NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_retention_files DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_retention_holds;
ALTER TABLE public.core_retention_holds NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_retention_holds DISABLE ROW LEVEL SECURITY;
