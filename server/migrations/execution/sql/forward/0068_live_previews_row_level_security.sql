ALTER TABLE public.core_live_previews ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_live_previews FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_live_previews AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_preview_limits ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_preview_limits FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_preview_limits AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.egress_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_sessions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
