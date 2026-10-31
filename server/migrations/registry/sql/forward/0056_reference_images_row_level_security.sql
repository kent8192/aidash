ALTER TABLE public.agent_knowledge_images ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_knowledge_images FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_knowledge_images AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
