-- Restores the Tenant Retention Rule tables as they stood after identity/0085,
-- with their keys, checks, index and forced row policies, empty. The rows are
-- not restored.
CREATE TABLE public.tenant_retention_rule_history (
    id uuid NOT NULL,
    tenant text NOT NULL,
    snapshot_after_run_seconds integer,
    snapshot_max_age_seconds integer,
    changed_at timestamp with time zone NOT NULL,
    changed_by text NOT NULL,
    CONSTRAINT tenant_retention_rule_history_periods CHECK ((((snapshot_after_run_seconds IS NULL) = (snapshot_max_age_seconds IS NULL)) AND ((snapshot_after_run_seconds IS NULL) OR (((snapshot_after_run_seconds >= 0) AND (snapshot_after_run_seconds <= 315360000)) AND ((snapshot_max_age_seconds >= 0) AND (snapshot_max_age_seconds <= 315360000))))))
);
ALTER TABLE ONLY public.tenant_retention_rule_history FORCE ROW LEVEL SECURITY;
CREATE TABLE public.tenant_retention_rules (
    tenant text NOT NULL,
    snapshot_after_run_seconds integer NOT NULL,
    snapshot_max_age_seconds integer NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    updated_by text NOT NULL,
    CONSTRAINT tenant_retention_rules_periods CHECK ((((snapshot_after_run_seconds >= 0) AND (snapshot_after_run_seconds <= 315360000)) AND ((snapshot_max_age_seconds >= 0) AND (snapshot_max_age_seconds <= 315360000))))
);
ALTER TABLE ONLY public.tenant_retention_rules FORCE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.tenant_retention_rule_history
    ADD CONSTRAINT tenant_retention_rule_history_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.tenant_retention_rules
    ADD CONSTRAINT tenant_retention_rules_pkey PRIMARY KEY (tenant);
CREATE INDEX idx_tenant_retention_rule_history_tenant_changed_at ON public.tenant_retention_rule_history USING btree (tenant, changed_at);
ALTER TABLE ONLY public.tenant_retention_rule_history
    ADD CONSTRAINT tenant_retention_rule_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.tenant_retention_rules
    ADD CONSTRAINT tenant_retention_rules_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
CREATE POLICY tenant_isolation ON public.tenant_retention_rule_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.tenant_retention_rules USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
ALTER TABLE public.tenant_retention_rule_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_retention_rules ENABLE ROW LEVEL SECURITY;
