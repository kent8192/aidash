-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit the core.schedules@1 builtins (#557) as descriptors and as default tools
-- an Agent may remove; retain the previous validators.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_schedules_0069(jsonb)') IS NOT NULL
 OR to_regprocedure('public.aidash_agent_before_schedules_0069(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'schedule contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_schedules_0069(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_schedules_0069(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' IS DISTINCT FROM 'core.schedules@1' THEN RETURN public.aidash_descriptor_is_valid_before_schedules_0069(value); END IF;
 IF value->>'operation' IS NULL OR value->>'operation' NOT IN ('schedule_create','schedule_list','schedule_update','schedule_delete')
  OR value->>'default_alias' IS DISTINCT FROM value->>'operation' THEN RETURN false; END IF;
 -- Reuse the established builtin shape, tier and no-transport rules, without relaxing them.
 RETURN public.aidash_descriptor_is_valid_before_schedules_0069(value || '{"provider":"core.tasks@1","operation":"task_create","default_alias":"task_create"}'::jsonb);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE removals jsonb;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF value ? 'remove_default' AND jsonb_typeof(value->'remove_default') = 'array' THEN
  removals := value->'remove_default';
  IF (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements(removals) e) THEN RETURN false; END IF;
  -- The `schedules` default tools may be removed like the earlier defaults;
  -- every other entry is still judged by the previous rules.
  value := jsonb_set(value, '{remove_default}', COALESCE((
   SELECT jsonb_agg(e ORDER BY o) FROM jsonb_array_elements(removals) WITH ORDINALITY AS r(e, o)
   WHERE NOT (jsonb_typeof(e) = 'string'
    AND e #>> '{}' IN ('schedule_create','schedule_list','schedule_update','schedule_delete'))), '[]'::jsonb));
 END IF;
 RETURN public.aidash_agent_before_schedules_0069(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
