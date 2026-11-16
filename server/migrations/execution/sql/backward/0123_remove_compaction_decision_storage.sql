CREATE TRIGGER decision_states_atomic BEFORE INSERT OR UPDATE OR DELETE ON public.decision_states FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
ALTER TABLE public.decision_states ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.decision_states FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.decision_states AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
CREATE TRIGGER decision_outcomes_atomic BEFORE INSERT OR UPDATE OR DELETE ON public.decision_outcomes FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
ALTER TABLE public.decision_outcomes ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.decision_outcomes FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.decision_outcomes AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
