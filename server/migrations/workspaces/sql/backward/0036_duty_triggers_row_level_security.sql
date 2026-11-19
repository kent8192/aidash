DROP POLICY tenant_isolation ON public.duty_bookings;
ALTER TABLE public.duty_bookings NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.duty_bookings DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.duty_events;
ALTER TABLE public.duty_events NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.duty_events DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.duty_triggers;
ALTER TABLE public.duty_triggers NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.duty_triggers DISABLE ROW LEVEL SECURITY;
