DROP TRIGGER decision_states_atomic ON public.decision_states;
DROP POLICY tenant_isolation ON public.decision_states;
DROP TRIGGER decision_outcomes_atomic ON public.decision_outcomes;
DROP POLICY tenant_isolation ON public.decision_outcomes;
