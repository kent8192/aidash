DROP POLICY tenant_isolation ON public.duty_deliveries;
ALTER TABLE public.duty_deliveries NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.duty_deliveries DISABLE ROW LEVEL SECURITY;
