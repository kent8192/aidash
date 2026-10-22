DROP POLICY IF EXISTS tenant_isolation ON public.egress_budgets;
ALTER TABLE public.egress_budgets DISABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON public.egress_requests;
ALTER TABLE public.egress_requests DISABLE ROW LEVEL SECURITY;
