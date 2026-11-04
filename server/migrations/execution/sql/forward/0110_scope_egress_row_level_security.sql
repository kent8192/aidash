ALTER TABLE public.egress_scope_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_scope_state FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_scope_state AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.egress_scope_daily ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_scope_daily FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_scope_daily AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.egress_scope_connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_scope_connections FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_scope_connections AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.egress_scope_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_scope_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_scope_events AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
