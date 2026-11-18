-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit the structured@4 Harness Contract (#1010). The previous validator
-- judges it exactly as structured@3, so nothing else is relaxed; as for
-- structured@3, registration checks the Native pairing, not this validator.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_agent_before_structured4_0070(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'structured@4 contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_structured4_0070(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF value->'harness_contract' = '"structured@4"'::jsonb THEN
  value := jsonb_set(value, '{harness_contract}', '"structured@3"'::jsonb);
 END IF;
 RETURN public.aidash_agent_before_structured4_0070(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
