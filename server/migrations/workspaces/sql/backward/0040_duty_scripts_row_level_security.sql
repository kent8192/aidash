DROP POLICY tenant_isolation ON public.duty_script_runs;
ALTER TABLE public.duty_script_runs NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.duty_script_runs DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.duty_script_versions;
ALTER TABLE public.duty_script_versions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.duty_script_versions DISABLE ROW LEVEL SECURITY;
