-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit the core.duties@1 builtins (#1011), the renamed core.schedules@1 tools,
-- with exactly the rules of the tools they replace, and let an Agent remove
-- them from its default tools; retain the previous validators.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_duties_0078(jsonb)') IS NOT NULL
 OR to_regprocedure('public.aidash_agent_before_duties_0078(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'duty contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_duties_0078(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_duties_0078(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' IS DISTINCT FROM 'core.duties@1' THEN RETURN public.aidash_descriptor_is_valid_before_duties_0078(value); END IF;
 IF value->>'operation' IS NULL OR value->>'operation' NOT IN ('duty_create','duty_list','duty_update','duty_delete')
  OR value->>'default_alias' IS DISTINCT FROM value->>'operation' THEN RETURN false; END IF;
 RETURN public.aidash_descriptor_is_valid_before_duties_0078(value || jsonb_build_object(
  'provider', 'core.schedules@1',
  'operation', replace(value->>'operation', 'duty_', 'schedule_'),
  'default_alias', replace(value->>'operation', 'duty_', 'schedule_')));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF value ? 'remove_default' AND jsonb_typeof(value->'remove_default') = 'array' THEN
  -- The Duty default tools are removed like the Schedule tools they replace.
  value := jsonb_set(value, '{remove_default}', COALESCE((
   SELECT jsonb_agg(CASE
     WHEN jsonb_typeof(e) = 'string' AND e #>> '{}' IN ('duty_create','duty_list','duty_update','duty_delete')
     THEN to_jsonb(replace(e #>> '{}', 'duty_', 'schedule_'))
     ELSE e END ORDER BY o)
   FROM jsonb_array_elements(value->'remove_default') WITH ORDINALITY AS r(e, o)), '[]'::jsonb));
 END IF;
 RETURN public.aidash_agent_before_duties_0078(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
