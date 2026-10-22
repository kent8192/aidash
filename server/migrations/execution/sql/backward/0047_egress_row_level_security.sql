DROP POLICY tenant_isolation ON public.egress_requests;
ALTER TABLE public.egress_requests NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.egress_requests DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.egress_budgets;
ALTER TABLE public.egress_budgets NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.egress_budgets DISABLE ROW LEVEL SECURITY;
