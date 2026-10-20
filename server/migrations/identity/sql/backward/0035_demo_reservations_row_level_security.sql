DROP POLICY tenant_isolation ON public.demo_reservations;
ALTER TABLE public.demo_reservations NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.demo_reservations DISABLE ROW LEVEL SECURITY;
