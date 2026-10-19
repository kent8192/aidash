ALTER TABLE public.memory_bank_decay ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_bank_decay FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_bank_decay AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_bank_policy_changes ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_bank_policy_changes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_bank_policy_changes AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_bank_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_bank_settings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_bank_settings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_candidate_reviews ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_candidate_reviews FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_candidate_reviews AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_candidates ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_candidates FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_candidates AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_dependencies ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_dependencies FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_dependencies AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_engine_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_engine_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_engine_jobs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_model_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_model_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_model_attempts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_model_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_model_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_model_operations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_publications ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_publications FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_publications AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_purge_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_purge_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_purge_jobs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_receipts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_receiver_caches ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_receiver_caches FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_receiver_caches AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_remote_read_gates ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_remote_read_gates FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_remote_read_gates AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_remote_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_remote_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_remote_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_run_bindings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_run_bindings FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_run_bindings AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_run_read_gates ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_run_read_gates FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_run_read_gates AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_run_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_run_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_run_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_task_participants ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_task_participants FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_task_participants AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_unit_origins ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_unit_origins FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_unit_origins AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_unit_retention ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_unit_retention FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_unit_retention AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_unit_run_origins ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_unit_run_origins FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_unit_run_origins AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.memory_units ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_units FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_units AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_collections ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_collections FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_collections AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_entries FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_entries AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_points ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_points FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_points AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_remote_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_remote_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_remote_attempts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_remote_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_remote_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_remote_operations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_remote_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_remote_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_remote_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_remote_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_remote_receipts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_remote_receipts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.semantic_run_reads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_run_reads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.semantic_run_reads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
