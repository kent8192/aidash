-- Row-level security policies have no typed migration operation. The tables'
-- own drops below remove the row-level-security flags with them.
DROP POLICY tenant_isolation ON public.egress_sessions;
DROP POLICY tenant_isolation ON public.egress_budgets;
