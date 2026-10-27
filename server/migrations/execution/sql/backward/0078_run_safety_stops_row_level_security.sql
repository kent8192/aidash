DROP POLICY tenant_isolation ON public.run_safety_stops;
ALTER TABLE public.run_safety_stops NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.run_safety_stops DISABLE ROW LEVEL SECURITY;
