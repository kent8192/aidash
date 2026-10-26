ALTER TABLE public.research_journal_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_journal_results FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_journal_results AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
