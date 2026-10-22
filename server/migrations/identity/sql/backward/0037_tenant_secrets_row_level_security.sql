DROP POLICY tenant_isolation ON public.tenant_secrets;
ALTER TABLE public.tenant_secrets NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_secrets DISABLE ROW LEVEL SECURITY;
