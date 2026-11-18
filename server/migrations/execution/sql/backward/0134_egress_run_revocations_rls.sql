DROP POLICY tenant_isolation ON public.egress_run_revocations;
ALTER TABLE public.egress_run_revocations NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.egress_run_revocations DISABLE ROW LEVEL SECURITY;
