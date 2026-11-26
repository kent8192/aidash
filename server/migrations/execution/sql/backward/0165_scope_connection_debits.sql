DROP POLICY tenant_isolation ON public.egress_usage_daily;
ALTER TABLE public.egress_usage_daily NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.egress_usage_daily DISABLE ROW LEVEL SECURITY;
