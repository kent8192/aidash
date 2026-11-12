-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit the core.schedules@1 builtins (#557); retain the previous validator.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_schedules_0064(jsonb)') IS NOT NULL THEN RAISE EXCEPTION 'schedule descriptor backup already exists'; END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_schedules_0064(');
 IF copied = original THEN RAISE EXCEPTION 'contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' IS DISTINCT FROM 'core.schedules@1' THEN RETURN public.aidash_descriptor_is_valid_before_schedules_0064(value); END IF;
 IF value->>'operation' IS NULL OR value->>'operation' NOT IN ('schedule_create','schedule_list','schedule_update','schedule_delete')
  OR value->>'default_alias' IS DISTINCT FROM value->>'operation' THEN RETURN false; END IF;
 -- Reuse the established builtin shape, tier and no-transport rules, without relaxing them.
 RETURN public.aidash_descriptor_is_valid_before_schedules_0064(value || '{"provider":"core.tasks@1","operation":"task_create","default_alias":"task_create"}'::jsonb);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
