DROP POLICY tenant_isolation ON public.core_retention_files;
ALTER TABLE public.core_retention_files NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_retention_files DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_quotas;
ALTER TABLE public.core_quotas NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_quotas DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_objects;
ALTER TABLE public.core_objects NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_objects DISABLE ROW LEVEL SECURITY;
