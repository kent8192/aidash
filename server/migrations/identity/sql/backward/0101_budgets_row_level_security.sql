DROP POLICY tenant_isolation ON public.budget_spend;
ALTER TABLE public.budget_spend NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.budget_spend DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.budget_notices;
ALTER TABLE public.budget_notices NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.budget_notices DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.budgets;
ALTER TABLE public.budgets NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.budgets DISABLE ROW LEVEL SECURITY;
