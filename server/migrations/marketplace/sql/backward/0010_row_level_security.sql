DROP POLICY tenant_isolation ON public.marketplace_installations;
ALTER TABLE public.marketplace_installations NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.marketplace_installations DISABLE ROW LEVEL SECURITY;
