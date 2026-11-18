DROP POLICY tenant_isolation ON public.stints;
ALTER TABLE public.stints NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.stints DISABLE ROW LEVEL SECURITY;
