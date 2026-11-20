DROP POLICY tenant_isolation ON public.duty_pending_events;
ALTER TABLE public.duty_pending_events NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.duty_pending_events DISABLE ROW LEVEL SECURITY;
