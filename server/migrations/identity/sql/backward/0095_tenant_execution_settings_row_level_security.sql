DROP POLICY tenant_isolation ON public.tenant_execution_settings;
ALTER TABLE public.tenant_execution_settings NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_execution_settings DISABLE ROW LEVEL SECURITY;
