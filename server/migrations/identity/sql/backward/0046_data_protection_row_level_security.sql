DROP POLICY tenant_isolation ON public.tenant_data_protection_history;
ALTER TABLE public.tenant_data_protection_history NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_data_protection_history DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.tenant_data_protection;
ALTER TABLE public.tenant_data_protection NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_data_protection DISABLE ROW LEVEL SECURITY;
