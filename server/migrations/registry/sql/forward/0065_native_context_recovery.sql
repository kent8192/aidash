-- PostgreSQL cannot express validation function bodies as typed migration operations.
DO $$ DECLARE original text; BEGIN
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 EXECUTE replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_native_recovery_0065(');
END $$;
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE policy jsonb; summary jsonb; field text; number numeric;
BEGIN
 policy := COALESCE(value->'context_policy', 'null'::jsonb);
 IF policy <> 'null'::jsonb THEN
  IF jsonb_typeof(policy) <> 'object' THEN RETURN false; END IF;
  FOREACH field IN ARRAY ARRAY['offload_target','protected_tail_fraction','reread_immunity_steps','reread_summary_threshold'] LOOP
   IF policy ? field THEN
    IF jsonb_typeof(policy->field) <> 'number' THEN RETURN false; END IF;
    number := (policy->>field)::numeric;
    IF (field = 'offload_target' AND number NOT BETWEEN 0.5 AND 0.9)
    OR (field = 'protected_tail_fraction' AND number NOT BETWEEN 0.05 AND 0.5)
    OR (field = 'reread_immunity_steps' AND (number <> trunc(number) OR number NOT BETWEEN 1 AND 100))
    OR (field = 'reread_summary_threshold' AND (number <> trunc(number) OR number NOT BETWEEN 2 AND 100)) THEN RETURN false; END IF;
   END IF;
  END LOOP;
  summary := COALESCE(policy->'summary', 'null'::jsonb);
  IF summary <> 'null'::jsonb THEN
   IF jsonb_typeof(summary) <> 'object' OR summary ? 'model' THEN RETURN false; END IF;
   policy := jsonb_set(policy, '{summary}', summary || jsonb_build_object('model', value->'model'));
  END IF;
  policy := policy - ARRAY['offload_target','protected_tail_fraction','reread_immunity_steps','reread_summary_threshold']::text[];
  value := jsonb_set(value, '{context_policy}', policy);
 END IF;
 RETURN public.aidash_agent_before_native_recovery_0065(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
