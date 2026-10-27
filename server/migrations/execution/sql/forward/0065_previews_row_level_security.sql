ALTER TABLE public.core_previews ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_previews FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_previews AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_preview_tickets ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_preview_tickets FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_preview_tickets AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_preview_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_preview_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_preview_sessions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
