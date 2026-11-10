SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE INDEX activation_dependency_runs ON public.runs USING btree (task_id) WHERE (phase <> ALL (ARRAY['COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text]));

--

CREATE INDEX activation_due ON public.run_activations USING btree (due_at, generation) WHERE (state <> 'settled'::text);

--

CREATE INDEX activation_run_generation ON public.run_activations USING btree (run_id, generation);

--

CREATE INDEX context_compaction_attempt_stage_lookup ON public.context_compaction_attempts USING btree (run_id, stage);

--

CREATE INDEX core_blob_references_object ON public.core_blob_references USING btree (tenant, object_id);

--

CREATE INDEX core_blob_references_record ON public.core_blob_references USING btree (tenant, record_id);

--

CREATE INDEX core_blob_references_revision ON public.core_blob_references USING btree (tenant, area_id, revision);

--

CREATE INDEX core_blob_uploads_tenant_digest ON public.core_blob_uploads USING btree (tenant, digest);

--

CREATE INDEX core_objects_tenant_digest ON public.core_objects USING btree (tenant, digest);

--

CREATE INDEX core_operation_pending_queue ON public.core_operations USING btree (updated_at) WHERE (state = ANY (ARRAY['prepared'::text, 'submitted'::text, 'running'::text, 'cancelling'::text]));

--

CREATE INDEX core_retention_files_object ON public.core_retention_files USING btree (tenant, file_id);

--

CREATE INDEX core_retention_holds_area ON public.core_retention_holds USING btree (tenant, area_id);

--

CREATE INDEX core_retention_holds_expiry ON public.core_retention_holds USING btree (expires_at);

--

CREATE INDEX core_retention_holds_holder ON public.core_retention_holds USING btree (tenant, holder_kind, holder_id);

--

CREATE INDEX dashboard_status_waiting_runs ON public.runs USING btree (id) WHERE ((control = 'PAUSED'::text) AND (error = 'identity status unavailable'::text));

--

CREATE INDEX decision_attempt_decision ON public.decision_attempts USING btree (decision_id);

--

CREATE INDEX decision_attempt_run_lookup ON public.decision_attempts USING btree (run_id);

--

CREATE INDEX decision_evidence_decision ON public.decision_evidence USING btree (decision_id, created_at);

--

CREATE INDEX decision_outcome_decision ON public.decision_outcomes USING btree (decision_id);

--

CREATE INDEX decision_state_expiry ON public.decision_states USING btree (expires_at);

--

CREATE INDEX egress_requests_run ON public.egress_requests USING btree (tenant, run_id, id);

--

CREATE INDEX events_marketplace_package_sequence ON public.events USING btree (((data ->> 'key'::text)), sequence) WHERE ((workspace_id IS NULL) AND (kind ~~ 'marketplace.%'::text) AND (kind <> 'marketplace.audit'::text));

--

CREATE INDEX events_marketplace_tenant_sequence ON public.events USING btree (((data ->> 'tenant'::text)), sequence) WHERE ((workspace_id IS NULL) AND (kind ~~ 'marketplace.%'::text) AND (kind <> 'marketplace.audit'::text));

--

CREATE INDEX events_outbox ON public.events USING btree (sequence) WHERE (published_at IS NULL);

--

CREATE INDEX events_retry ON public.events USING btree (next_attempt_at, sequence) WHERE (published_at IS NULL);

--

CREATE INDEX events_tool_run ON public.events USING btree (((data ->> 'run_id'::text))) WHERE (kind = ANY (ARRAY['tool.started'::text, 'tool.completed'::text]));

--

CREATE INDEX events_workspace ON public.events USING btree (workspace_id, sequence);

--

CREATE INDEX generation_remote_dispatches_pending_finalization ON public.generation_remote_dispatches USING btree (created_at) WHERE ((state = ANY (ARRAY['ABORTED'::text, 'SETTLED'::text])) AND (peer_finalized = false));

--

CREATE INDEX generation_remote_dispatches_preparing ON public.generation_remote_dispatches USING btree (created_at) WHERE (state = 'PREPARING'::text);

--

CREATE INDEX generation_remote_intents_cancel_retry ON public.generation_remote_intents USING btree (cancelled, cancel_delivered, cancel_retry_at);

--

CREATE INDEX generation_remote_usage_attempt_digest ON public.generation_remote_usage USING btree (attempt_id, digest);

--

CREATE INDEX generation_requests_policy ON public.generation_requests USING btree (tenant, policy_id, status);

--

CREATE INDEX human_answered_created_page ON public.human_requests USING btree (tenant, created_at, id) WHERE (response IS NOT NULL);

--

CREATE INDEX human_created_page ON public.human_requests USING btree (tenant, created_at, id);

--

CREATE INDEX human_pending_created_page ON public.human_requests USING btree (tenant, created_at, id) WHERE (response IS NULL);

--

CREATE INDEX human_run_answered_created_page ON public.human_requests USING btree (tenant, run_id, created_at, id) WHERE (response IS NOT NULL);

--

CREATE INDEX human_run_created_page ON public.human_requests USING btree (tenant, run_id, created_at, id);

--

CREATE INDEX human_run_pending_created_page ON public.human_requests USING btree (tenant, run_id, created_at, id) WHERE (response IS NULL);

--

CREATE INDEX idx_core_live_previews_tenant_run_id_started_at ON public.core_live_previews USING btree (tenant, run_id, started_at);

--

CREATE INDEX idx_egress_scope_connections_tenant_node_scope_id ON public.egress_scope_connections USING btree (tenant, node, scope_id);

--

CREATE INDEX idx_egress_scope_events_created_at_id_011bbb392887b28a ON public.egress_scope_events USING btree (created_at, id) WHERE (delivered_at IS NULL);

--

CREATE INDEX idx_egress_scope_events_tenant_node_scope_id ON public.egress_scope_events USING btree (tenant, node, scope_id);

--

CREATE INDEX idx_estimate_calibration_samples_upstream_mode_4fd2ade9d4cc0c95 ON public.estimate_calibration_samples USING btree (upstream_model, projection_version, completed_at);

--

CREATE INDEX idx_inference_usage_run_id_outcome ON public.inference_usage USING btree (run_id, outcome);

--

CREATE INDEX idx_research_attempts_account_id_created_at_d4f5ed98697a2315 ON public.research_attempts USING btree (account_id, created_at) WHERE (account_id IS NOT NULL);

--

CREATE INDEX idx_research_attempts_id_c91d99759e4ae2e3 ON public.research_attempts USING btree (id) WHERE ((retention_account_id IS NOT NULL) AND (purged_at IS NULL));

--

CREATE INDEX idx_research_originals_run_id ON public.research_originals USING btree (run_id);

--

CREATE INDEX idx_research_page_images_run_id ON public.research_page_images USING btree (run_id);

--

CREATE INDEX idx_research_revocations_tenant_revoked_at_39caea2f02a3c4fc ON public.research_revocations USING btree (tenant, revoked_at) WHERE (tenant IS NOT NULL);

--

CREATE INDEX idx_research_snapshots_id_f1769031a31290d4 ON public.research_snapshots USING btree (id) WHERE (purged_at IS NULL);

--

CREATE INDEX idx_research_snapshots_run_id_attempt_key ON public.research_snapshots USING btree (run_id, attempt_key);

--

CREATE INDEX idx_research_sources_run_id_attempt_key ON public.research_sources USING btree (run_id, attempt_key);

--

CREATE INDEX idx_run_safety_stops_tenant_created_at_id ON public.run_safety_stops USING btree (tenant, created_at, id);

--

CREATE INDEX idx_run_safety_stops_tenant_run_id ON public.run_safety_stops USING btree (tenant, run_id);

--

CREATE INDEX idx_url_intents_run_id ON public.url_intents USING btree (run_id);

--

CREATE INDEX inference_attempt_retention ON public.inference_attempts USING btree (finished_at) WHERE ((outcome IS NOT NULL) AND (pruned_at IS NULL));

--

CREATE INDEX inference_attempt_run_sequence ON public.inference_attempts USING btree (run_id, first_seq);

--

CREATE INDEX inference_progress_attempt ON public.inference_progress USING btree (attempt_id, seq);

--

CREATE INDEX node_cancellation_jobs_due ON public.node_cancellation_jobs USING btree (retry_at, id);

--

CREATE INDEX runs_ready ON public.runs USING btree (updated_at) WHERE (phase <> ALL (ARRAY['COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text]));

--

CREATE INDEX runs_task_created_page ON public.runs USING btree (tenant, task_id, created_at, id);

--

CREATE TRIGGER aidash_activation AFTER UPDATE OF state ON public.core_areas FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();

--

CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE OF state, expires_at ON public.core_records FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();

--

CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE OF initialized ON public.core_runs FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();

--

CREATE TRIGGER aidash_activation AFTER UPDATE OF response ON public.human_requests FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();

--

CREATE TRIGGER aidash_activation AFTER INSERT ON public.run_inputs FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();

--

CREATE TRIGGER aidash_activation AFTER INSERT OR UPDATE ON public.runs FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();

--

CREATE TRIGGER aidash_operation_finished_activation AFTER UPDATE OF state ON public.core_operations FOR EACH ROW EXECUTE FUNCTION public.aidash_operation_finished_activation();

--

CREATE TRIGGER aidash_run_control_provenance BEFORE UPDATE OF control ON public.runs FOR EACH ROW EXECUTE FUNCTION public.aidash_run_control_provenance();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_areas FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_objects FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_operations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_quotas FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_records FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_runs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_task_sessions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.events FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_budgets FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_compaction_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_embedding_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_policies FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_policy_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_dispatches FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_finalizations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_intents FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_remote_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_summary_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.generation_usage FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.human_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.inbox FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.inference_attempts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.inference_progress FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.invocations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.run_inputs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.runs FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER context_compaction_attempts_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.context_compaction_attempts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER decision_attempts_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.decision_attempts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER decision_evidence_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.decision_evidence FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER decision_outcomes_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.decision_outcomes FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER decision_states_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.decision_states FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER gate_legacy_run_worker BEFORE UPDATE ON public.runs FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_run_worker();

--

CREATE TRIGGER run_context_events_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.run_context_events FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER run_task_snapshots_atomic BEFORE INSERT OR DELETE OR UPDATE ON public.run_task_snapshots FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

ALTER TABLE ONLY public.approval_observation_attempts
    ADD CONSTRAINT approval_observation_attempts_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.approval_observations
    ADD CONSTRAINT approval_observations_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.approval_observer_settings
    ADD CONSTRAINT approval_observer_settings_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.approval_observation_population
    ADD CONSTRAINT approval_population_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.context_compaction_attempts
    ADD CONSTRAINT context_compaction_attempts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_areas
    ADD CONSTRAINT core_areas_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_blob_references
    ADD CONSTRAINT core_blob_references_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_blob_uploads
    ADD CONSTRAINT core_blob_uploads_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_blobs
    ADD CONSTRAINT core_blobs_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_live_previews
    ADD CONSTRAINT core_live_previews_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_manifest_revisions
    ADD CONSTRAINT core_manifest_revisions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_objects
    ADD CONSTRAINT core_objects_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_operations
    ADD CONSTRAINT core_operations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_preview_limits
    ADD CONSTRAINT core_preview_limits_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_preview_sessions
    ADD CONSTRAINT core_preview_sessions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_preview_tickets
    ADD CONSTRAINT core_preview_tickets_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_previews
    ADD CONSTRAINT core_previews_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_quotas
    ADD CONSTRAINT core_quotas_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_records
    ADD CONSTRAINT core_records_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_requests
    ADD CONSTRAINT core_requests_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_retention_files
    ADD CONSTRAINT core_retention_files_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_retention_holds
    ADD CONSTRAINT core_retention_holds_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_runs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.core_task_sessions
    ADD CONSTRAINT core_task_sessions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.decision_attempts
    ADD CONSTRAINT decision_attempts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.decision_evidence
    ADD CONSTRAINT decision_evidence_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.decision_outcomes
    ADD CONSTRAINT decision_outcomes_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.decision_states
    ADD CONSTRAINT decision_states_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.egress_budgets
    ADD CONSTRAINT egress_budgets_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.egress_requests
    ADD CONSTRAINT egress_requests_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.egress_scope_connections
    ADD CONSTRAINT egress_scope_connections_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.egress_scope_daily
    ADD CONSTRAINT egress_scope_daily_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.egress_scope_events
    ADD CONSTRAINT egress_scope_events_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.egress_scope_state
    ADD CONSTRAINT egress_scope_state_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.egress_scope_state
    ADD CONSTRAINT egress_scope_state_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.egress_sessions
    ADD CONSTRAINT egress_sessions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_node_fkey FOREIGN KEY (node, tenant) REFERENCES public.node_names(name, tenant);

--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.generation_budgets
    ADD CONSTRAINT generation_budgets_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.generation_history
    ADD CONSTRAINT generation_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_policies
    ADD CONSTRAINT generation_policies_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.generation_policies
    ADD CONSTRAINT generation_policies_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_policy_history
    ADD CONSTRAINT generation_policy_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_remote_dispatches
    ADD CONSTRAINT generation_remote_dispatches_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_remote_finalizations
    ADD CONSTRAINT generation_remote_finalizations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_remote_intents
    ADD CONSTRAINT generation_remote_intents_credential_id_fkey FOREIGN KEY (tenant, credential_id) REFERENCES public.authorization_credentials(tenant, id);

--

ALTER TABLE ONLY public.generation_remote_intents
    ADD CONSTRAINT generation_remote_intents_task_id_fkey FOREIGN KEY (tenant, task_id) REFERENCES public.tasks(tenant, id);

--

ALTER TABLE ONLY public.generation_remote_intents
    ADD CONSTRAINT generation_remote_intents_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_remote_usage
    ADD CONSTRAINT generation_remote_usage_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_credential_id_fkey FOREIGN KEY (tenant, credential_id) REFERENCES public.authorization_credentials(tenant, id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_local_task_id_fkey FOREIGN KEY (tenant, local_task_id) REFERENCES public.tasks(tenant, id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_local_workspace_id_fkey FOREIGN KEY (tenant, local_workspace_id) REFERENCES public.authorization_workspaces(tenant, workspace_id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_task_workspace FOREIGN KEY (tenant, local_task_id, local_workspace_id) REFERENCES public.tasks(tenant, id, workspace_id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_summary_usage
    ADD CONSTRAINT generation_summary_usage_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.generation_usage
    ADD CONSTRAINT generation_usage_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.inference_attempts
    ADD CONSTRAINT inference_attempts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.inference_progress
    ADD CONSTRAINT inference_progress_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.inference_usage
    ADD CONSTRAINT inference_usage_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.invocations
    ADD CONSTRAINT invocations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.node_cancellation_jobs
    ADD CONSTRAINT node_cancellation_jobs_deletion_owner_fkey FOREIGN KEY (deletion_id, tenant, node) REFERENCES public.node_deletions(id, tenant, node);

--

ALTER TABLE ONLY public.node_cancellation_jobs
    ADD CONSTRAINT node_cancellation_jobs_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_originals
    ADD CONSTRAINT research_originals_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_page_images
    ADD CONSTRAINT research_page_images_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_revocations
    ADD CONSTRAINT research_revocations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_activations
    ADD CONSTRAINT run_activations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_context_events
    ADD CONSTRAINT run_context_events_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_inputs
    ADD CONSTRAINT run_inputs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_safety_stops
    ADD CONSTRAINT run_safety_stops_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_task_snapshots
    ADD CONSTRAINT run_task_snapshots_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name);

--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.subnode_limits
    ADD CONSTRAINT subnode_limits_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.subnode_members
    ADD CONSTRAINT subnode_members_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.subnode_operation_receipts
    ADD CONSTRAINT subnode_operation_receipts_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.subnodes
    ADD CONSTRAINT subnodes_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.subnodes
    ADD CONSTRAINT subnodes_workspace_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.url_intents
    ADD CONSTRAINT url_intents_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE public.approval_observation_attempts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.approval_observation_population ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.approval_observations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.approval_observer_settings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.context_compaction_attempts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_areas ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_blob_references ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_blob_uploads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_blobs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_live_previews ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_manifest_revisions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_objects ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_operations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_preview_limits ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_preview_sessions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_preview_tickets ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_previews ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_quotas ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_records ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_requests ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_retention_files ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_retention_holds ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_runs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.core_task_sessions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.decision_attempts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.decision_evidence ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.decision_outcomes ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.decision_states ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.egress_budgets ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.egress_requests ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.egress_scope_connections ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.egress_scope_daily ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.egress_scope_events ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.egress_scope_state ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.egress_sessions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.events ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_budgets ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_compaction_usage ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_embedding_usage ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_policies ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_policy_history ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_remote_dispatches ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_remote_finalizations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_remote_intents ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_remote_usage ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_requests ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_summary_usage ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_usage ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.human_requests ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.inference_attempts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.inference_progress ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.inference_usage ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.invocations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.node_cancellation_jobs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_attempts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_fragments ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_journal_results ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_originals ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_page_images ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_revocations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_snapshots ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.research_sources ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.run_activations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.run_context_events ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.run_inputs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.run_safety_stops ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.run_task_snapshots ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.runs ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.subnode_limits ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.subnode_members ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.subnode_operation_receipts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.subnodes ENABLE ROW LEVEL SECURITY;

--

CREATE POLICY tenant_isolation ON public.approval_observation_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.approval_observation_population USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.approval_observations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.approval_observer_settings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.context_compaction_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_areas USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_blob_references USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_blob_uploads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_blobs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_live_previews USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_manifest_revisions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_objects USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_operations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_preview_limits USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_preview_sessions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_preview_tickets USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_previews USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_quotas USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_records USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_requests USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_retention_files USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_retention_holds USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_runs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.core_task_sessions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.decision_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.decision_evidence USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.decision_outcomes USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.decision_states USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.egress_budgets USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.egress_requests USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.egress_scope_connections USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.egress_scope_daily USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.egress_scope_events USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.egress_scope_state USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.egress_sessions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.events USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_budgets USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_compaction_usage USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_embedding_usage USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_policies USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_policy_history USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_remote_dispatches USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_remote_finalizations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_remote_intents USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_remote_usage USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_requests USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_summary_usage USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.generation_usage USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.human_requests USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.inference_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.inference_progress USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.inference_usage USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.invocations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.node_cancellation_jobs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_fragments USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_journal_results USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_originals USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_page_images USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_revocations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_snapshots USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.research_sources USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.run_activations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.run_context_events USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.run_inputs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.run_safety_stops USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.run_task_snapshots USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.runs USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.subnode_limits USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.subnode_members USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.subnode_operation_receipts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.subnodes USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.url_intents USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

ALTER TABLE public.url_intents ENABLE ROW LEVEL SECURITY;
