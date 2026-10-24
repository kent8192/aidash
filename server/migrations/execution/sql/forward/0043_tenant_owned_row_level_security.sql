ALTER TABLE public.context_compaction_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.context_compaction_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.context_compaction_attempts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_runs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_runs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.core_task_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_task_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_task_sessions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.decision_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.decision_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.decision_attempts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.decision_evidence ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.decision_evidence FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.decision_evidence AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.decision_outcomes ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.decision_outcomes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.decision_outcomes AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.decision_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.decision_states FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.decision_states AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.events AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_budgets ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_budgets FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_budgets AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_compaction_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_compaction_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_compaction_usage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_embedding_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_embedding_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_embedding_usage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_history FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_history AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_remote_dispatches ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_remote_dispatches FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_remote_dispatches AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_remote_finalizations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_remote_finalizations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_remote_finalizations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_remote_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_remote_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_remote_usage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_summary_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_summary_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_summary_usage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.generation_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.generation_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.generation_usage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.human_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.human_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.human_requests AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.inference_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.inference_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.inference_attempts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.inference_progress ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.inference_progress FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.inference_progress AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.inference_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.inference_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.inference_usage AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.invocations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.invocations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.invocations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.research_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_attempts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.research_fragments ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_fragments FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_fragments AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.research_originals ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_originals FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_originals AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.research_page_images ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_page_images FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_page_images AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.research_snapshots ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_snapshots FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_snapshots AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.research_sources ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_sources FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_sources AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.run_activations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.run_activations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.run_activations AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.run_context_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.run_context_events FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.run_context_events AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.run_inputs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.run_inputs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.run_inputs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.run_task_snapshots ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.run_task_snapshots FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.run_task_snapshots AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.runs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.runs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.url_intents ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.url_intents FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.url_intents AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
