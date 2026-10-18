ALTER TABLE public.authorization_peer_mapping_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_peer_mapping_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_peer_mapping_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_peer_mappings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_peer_mappings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_peer_mappings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_remote_admissions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_admissions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_remote_admissions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_remote_grants ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_grants FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_remote_grants AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
