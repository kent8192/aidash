ALTER TABLE public.duty_pending_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.duty_pending_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.duty_pending_events AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
