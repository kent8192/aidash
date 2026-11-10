SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE INDEX idx_memory_run_reads_unit_id_run_id_revision ON public.memory_run_reads USING btree (unit_id, run_id, revision);

--

CREATE INDEX idx_memory_unit_retention_bank_id_dormant_policy_pinned ON public.memory_unit_retention USING btree (bank_id, dormant_policy, pinned);

--

CREATE INDEX idx_semantic_image_publications_workspace_id_sequence ON public.semantic_image_publications USING btree (workspace_id, sequence);

--

CREATE INDEX memory_units_full_text ON public.memory_units USING pgroonga (text);

--

CREATE INDEX semantic_remote_operation_run ON public.semantic_remote_operations USING btree (home_node, grant_id, admission_id);

--

CREATE INDEX semantic_remote_receipts_run ON public.semantic_remote_receipts USING btree (run_id);

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_banks FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_candidates FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_dependencies FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_participants FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_receipts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_run_bindings FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory_units FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_collections FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_entries FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_indexes FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_points FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_remote_attempts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_remote_operations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_remote_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_remote_receipts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_run_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_bank_decay_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_bank_decay FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_bank_policy_changes_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_bank_policy_changes FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_bank_settings_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_bank_settings FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_candidate_reviews_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_candidate_reviews FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_engine_jobs_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_engine_jobs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_learning_discovery_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_learning_discovery FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_model_attempts_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_model_attempts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_model_operations_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_model_operations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_publications_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_publications FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_purge_jobs_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_purge_jobs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_receiver_caches_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_receiver_caches FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_remote_read_gates_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_remote_read_gates FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_remote_reads_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_remote_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_run_read_gates_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_run_read_gates FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_run_reads_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_run_reads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_task_participants_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_task_participants FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_unit_origins_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_unit_origins FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_unit_retention_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_unit_retention FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER memory_unit_run_origins_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.memory_unit_run_origins FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER semantic_index_input_limit_guard BEFORE INSERT OR UPDATE OF workspace_id, spec ON public.semantic_indexes FOR EACH ROW EXECUTE FUNCTION public.guard_semantic_index_input_limit();

--

CREATE TRIGGER semantic_memory_entry_bytes_guard BEFORE INSERT OR UPDATE OF workspace_id, source, deleted ON public.semantic_entries FOR EACH ROW EXECUTE FUNCTION public.guard_semantic_memory_entry_bytes();

--

ALTER TABLE ONLY public.memory_task_participants
    ADD CONSTRAINT memory_assignment_task FOREIGN KEY (tenant, task_id) REFERENCES public.tasks(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_bank_decay
    ADD CONSTRAINT memory_bank_decay_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_bank_policy_changes
    ADD CONSTRAINT memory_bank_policy_changes_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_bank_settings
    ADD CONSTRAINT memory_bank_settings_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_banks
    ADD CONSTRAINT memory_bank_workspace FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.memory_banks
    ADD CONSTRAINT memory_banks_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_run_bindings
    ADD CONSTRAINT memory_binding_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.memory_candidate_reviews
    ADD CONSTRAINT memory_candidate_reviews_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_candidates
    ADD CONSTRAINT memory_candidates_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_dependencies
    ADD CONSTRAINT memory_dependencies_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_engine_jobs
    ADD CONSTRAINT memory_engine_jobs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_history
    ADD CONSTRAINT memory_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_model_attempts
    ADD CONSTRAINT memory_model_attempts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_model_operations
    ADD CONSTRAINT memory_model_operations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_unit_origins
    ADD CONSTRAINT memory_origin_run FOREIGN KEY (tenant, origin_run) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_participants
    ADD CONSTRAINT memory_participant_agent FOREIGN KEY (tenant, agent_id, agent_version) REFERENCES public.registry(tenant, id, version) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_participants
    ADD CONSTRAINT memory_participant_workspace FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.memory_participants
    ADD CONSTRAINT memory_participants_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_publications
    ADD CONSTRAINT memory_publications_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_purge_jobs
    ADD CONSTRAINT memory_purge_jobs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_run_read_gates
    ADD CONSTRAINT memory_read_gate_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_run_reads
    ADD CONSTRAINT memory_read_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_receipts
    ADD CONSTRAINT memory_receipts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_receiver_caches
    ADD CONSTRAINT memory_receiver_cache_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_receiver_caches
    ADD CONSTRAINT memory_receiver_caches_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_remote_read_gates
    ADD CONSTRAINT memory_remote_read_gate_grant FOREIGN KEY (tenant, grant_id) REFERENCES public.authorization_remote_grants(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_remote_read_gates
    ADD CONSTRAINT memory_remote_read_gates_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_remote_reads
    ADD CONSTRAINT memory_remote_read_grant FOREIGN KEY (tenant, grant_id) REFERENCES public.authorization_remote_grants(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_remote_reads
    ADD CONSTRAINT memory_remote_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_run_bindings
    ADD CONSTRAINT memory_run_bindings_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_run_read_gates
    ADD CONSTRAINT memory_run_read_gates_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_run_reads
    ADD CONSTRAINT memory_run_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_task_participants
    ADD CONSTRAINT memory_task_participants_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_unit_origins
    ADD CONSTRAINT memory_unit_origins_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_unit_retention
    ADD CONSTRAINT memory_unit_retention_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_unit_run_origins
    ADD CONSTRAINT memory_unit_run_origin_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_unit_run_origins
    ADD CONSTRAINT memory_unit_run_origins_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.memory_units
    ADD CONSTRAINT memory_units_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_collections
    ADD CONSTRAINT semantic_collections_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_history
    ADD CONSTRAINT semantic_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_image_ledgers
    ADD CONSTRAINT semantic_image_ledgers_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_image_ledgers
    ADD CONSTRAINT semantic_image_ledgers_workspace_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.semantic_image_publications
    ADD CONSTRAINT semantic_image_publications_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_image_publications
    ADD CONSTRAINT semantic_image_publications_workspace_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.semantic_points
    ADD CONSTRAINT semantic_points_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_remote_attempts
    ADD CONSTRAINT semantic_remote_attempts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_remote_operations
    ADD CONSTRAINT semantic_remote_operations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_remote_reads
    ADD CONSTRAINT semantic_remote_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_remote_receipts
    ADD CONSTRAINT semantic_remote_receipts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_run_reads
    ADD CONSTRAINT semantic_run_reads_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.semantic_run_reads
    ADD CONSTRAINT semantic_run_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.semantic_vectors
    ADD CONSTRAINT semantic_vectors_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE public.memory_bank_decay ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_bank_policy_changes ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_bank_settings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_banks ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_candidate_reviews ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_candidates ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_dependencies ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_engine_jobs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_model_attempts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_model_operations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_participants ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_publications ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_purge_jobs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_receipts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_receiver_caches ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_remote_read_gates ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_remote_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_run_bindings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_run_read_gates ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_run_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_task_participants ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_unit_origins ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_unit_retention ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_unit_run_origins ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.memory_units ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_collections ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_entries ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_image_ledgers ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_image_publications ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_indexes ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_points ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_remote_attempts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_remote_operations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_remote_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_remote_receipts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_run_reads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_vectors ENABLE ROW LEVEL SECURITY;

--

CREATE POLICY tenant_isolation ON public.memory_bank_decay USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_bank_policy_changes USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_bank_settings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_banks USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_candidate_reviews USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_candidates USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_dependencies USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_engine_jobs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_model_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_model_operations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_participants USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_publications USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_purge_jobs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_receipts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_receiver_caches USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_remote_read_gates USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_remote_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_run_bindings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_run_read_gates USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_run_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_task_participants USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_unit_origins USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_unit_retention USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_unit_run_origins USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.memory_units USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_collections USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_entries USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_image_ledgers USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_image_publications USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_indexes USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_points USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_remote_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_remote_operations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_remote_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_remote_receipts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_run_reads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.semantic_vectors USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
