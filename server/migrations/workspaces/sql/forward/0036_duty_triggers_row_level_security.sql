ALTER TABLE public.duty_triggers ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.duty_triggers FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.duty_triggers AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.duty_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.duty_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.duty_events AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.duty_bookings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.duty_bookings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.duty_bookings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
