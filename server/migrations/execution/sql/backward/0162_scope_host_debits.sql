DROP POLICY tenant_isolation ON public.egress_scope_host_debits;
ALTER TABLE public.egress_scope_host_debits NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.egress_scope_host_debits DISABLE ROW LEVEL SECURITY;
