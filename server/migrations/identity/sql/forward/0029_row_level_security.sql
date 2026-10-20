ALTER TABLE public.authorization_bundles ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_bundles FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_bundles AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_catalog ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_catalog FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_catalog AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_catalog_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_catalog_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_catalog_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_credentials FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_credentials AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_decisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_decisions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_decisions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_execution ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_execution FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_execution AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_peer_graph_grants ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_peer_graph_grants FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_peer_graph_grants AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_revisions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_task_origins ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_task_origins FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_task_origins AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.authorization_workspaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_workspaces FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.authorization_workspaces AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.credential_store_resources ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.credential_store_resources FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.credential_store_resources AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.credential_store_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.credential_store_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.credential_store_versions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.dashboard_administration_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.dashboard_administration_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.dashboard_administration_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.dashboard_mappings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.dashboard_mappings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.dashboard_mappings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.identity_approval_preferences ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.identity_approval_preferences FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.identity_approval_preferences AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.identity_approval_rules ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.identity_approval_rules FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.identity_approval_rules AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.identity_thread_approval_modes ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.identity_thread_approval_modes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.identity_thread_approval_modes AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.personal_tenants ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.personal_tenants FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.personal_tenants AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.plan_connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.plan_connections FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.plan_connections AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.provider_credential_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.provider_credential_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.provider_credential_bindings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.provider_credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.provider_credentials FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.provider_credentials AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.run_plan_connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.run_plan_connections FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.run_plan_connections AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.run_provider_credentials ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.run_provider_credentials FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.run_provider_credentials AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.tenant_invitations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_invitations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.tenant_invitations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.tenant_retention_rule_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_retention_rule_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.tenant_retention_rule_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.tenant_retention_rules ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_retention_rules FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.tenant_retention_rules AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
