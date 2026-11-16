DROP POLICY tenant_isolation ON public.schedule_limits;
ALTER TABLE public.schedule_limits NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.schedule_limits DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.schedule_ticks;
ALTER TABLE public.schedule_ticks NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.schedule_ticks DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.schedules;
ALTER TABLE public.schedules NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.schedules DISABLE ROW LEVEL SECURITY;
