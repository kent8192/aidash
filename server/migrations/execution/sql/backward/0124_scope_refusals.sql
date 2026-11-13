DROP POLICY tenant_isolation ON public.egress_scope_refusals;
ALTER TABLE public.egress_scope_refusals NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.egress_scope_refusals DISABLE ROW LEVEL SECURITY;
