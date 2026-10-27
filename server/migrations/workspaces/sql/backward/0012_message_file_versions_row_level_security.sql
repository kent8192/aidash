DROP POLICY tenant_isolation ON public.message_file_versions;
ALTER TABLE public.message_file_versions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.message_file_versions DISABLE ROW LEVEL SECURITY;
