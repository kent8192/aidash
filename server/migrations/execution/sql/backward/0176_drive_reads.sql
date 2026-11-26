DROP POLICY tenant_isolation ON public.drive_run_reads;
ALTER TABLE public.drive_run_reads NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.drive_run_reads DISABLE ROW LEVEL SECURITY;
