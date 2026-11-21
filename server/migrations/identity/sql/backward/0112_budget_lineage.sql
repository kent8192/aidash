DROP POLICY tenant_isolation ON public.budget_lineage;
ALTER TABLE public.budget_lineage NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.budget_lineage DISABLE ROW LEVEL SECURITY;
