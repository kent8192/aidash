ALTER TABLE public.memory_journal_proofs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.memory_journal_proofs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.memory_journal_proofs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
