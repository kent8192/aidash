SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE INDEX channel_attachment_message_lookup ON public.channel_attachments USING btree (workspace_id, message_id);

--

CREATE INDEX channel_message_history_order ON public.messages USING btree (workspace_id, created_at, id);

--

CREATE INDEX channel_reply_lookup ON public.channel_message_context USING btree (workspace_id, thread_id, message_id);

--

CREATE INDEX idx_conversations_tenant_scratch_space_id ON public.conversations USING btree (tenant, scratch_space_id);

--

CREATE INDEX idx_scratch_spaces_tenant_node ON public.scratch_spaces USING btree (tenant, node);

--

CREATE INDEX idx_workspaces_tenant_scratch_space_id ON public.workspaces USING btree (tenant, scratch_space_id);

--

CREATE INDEX message_file_versions_message ON public.message_file_versions USING btree (tenant, message_id);

--

CREATE INDEX task_dependencies_target ON public.task_dependencies USING btree (dependency_id, workspace_id);

--

CREATE INDEX tasks_workspace ON public.tasks USING btree (workspace_id, created_at);

--

CREATE TRIGGER aidash_activation AFTER UPDATE OF status ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.aidash_activation_trigger();

--

CREATE TRIGGER aidash_workspace_task_activation AFTER UPDATE OF status ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.aidash_workspace_task_activation();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.artifacts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.channel_attachments FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.channel_message_context FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.channel_threads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.conversations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.messages FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.tasks FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.workspaces FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();

--

CREATE TRIGGER gate_legacy_federated_run_message BEFORE INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_federated_run_message();

--

CREATE TRIGGER gate_legacy_run_message BEFORE INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_run_message();

--

CREATE TRIGGER gate_legacy_run_output BEFORE INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.gate_legacy_run_output();

--

CREATE TRIGGER gate_remote_task_terminal BEFORE UPDATE OF status ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.gate_remote_task_terminal();

--

CREATE TRIGGER legacy_run_input_bridge AFTER INSERT ON public.messages FOR EACH ROW EXECUTE FUNCTION public.legacy_run_input_bridge();

--

CREATE TRIGGER messages_release_retention AFTER DELETE ON public.messages FOR EACH ROW EXECUTE FUNCTION public.aidash_release_message_retention();

--

CREATE TRIGGER task_dependencies_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.task_dependencies FOR EACH STATEMENT EXECUTE FUNCTION public.guard_task_dependencies();

--

CREATE TRIGGER tasks_dependencies_sync AFTER INSERT OR UPDATE OF id, workspace_id, dependencies ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.sync_task_dependencies();

--

CREATE TRIGGER tasks_hierarchy_serialize BEFORE INSERT OR UPDATE OF id, parent_id, workspace_id, dependencies ON public.tasks FOR EACH STATEMENT EXECUTE FUNCTION public.lock_task_hierarchy_before_change();

--

CREATE CONSTRAINT TRIGGER tasks_parent_cycle_guard AFTER INSERT OR UPDATE OF id, parent_id, workspace_id ON public.tasks DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.guard_task_parent_cycle();

--

CREATE CONSTRAINT TRIGGER zz_tasks_dependency_cycle_guard AFTER INSERT OR UPDATE OF id, parent_id, dependencies ON public.tasks DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.guard_task_dependency_cycle();

--

ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_object_fkey FOREIGN KEY (tenant, object_id) REFERENCES public.core_objects(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_wav_object_fkey FOREIGN KEY (tenant, wav_object_id) REFERENCES public.core_objects(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.channel_message_context
    ADD CONSTRAINT channel_message_context_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.message_file_versions
    ADD CONSTRAINT message_file_versions_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.scratch_space_members
    ADD CONSTRAINT scratch_space_members_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.scratch_space_settings
    ADD CONSTRAINT scratch_space_settings_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.scratch_spaces
    ADD CONSTRAINT scratch_spaces_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.scratch_spaces
    ADD CONSTRAINT scratch_spaces_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT task_dependencies_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name);

--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE public.artifacts ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.channel_attachments ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.channel_message_context ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.channel_threads ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.conversations ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.message_file_versions ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.messages ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.scratch_space_members ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.scratch_space_settings ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.scratch_spaces ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.task_dependencies ENABLE ROW LEVEL SECURITY;

--

ALTER TABLE public.tasks ENABLE ROW LEVEL SECURITY;

--

CREATE POLICY tenant_isolation ON public.artifacts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.channel_attachments USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.channel_message_context USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.channel_threads USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.conversations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.message_file_versions USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.messages USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.scratch_space_members USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.scratch_space_settings USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.scratch_spaces USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.task_dependencies USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.tasks USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

CREATE POLICY tenant_isolation ON public.workspaces USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));

--

ALTER TABLE public.workspaces ENABLE ROW LEVEL SECURITY;
