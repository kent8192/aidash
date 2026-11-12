-- Validation function DDL; do not invalidate stored Schedule tool descriptors or Agents.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' = 'core.schedules@1') THEN
  RAISE EXCEPTION 'registry 0064_schedule_tools cannot be reversed: definitions declare Schedule tools'
   USING ERRCODE = '55000';
 END IF;
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'agent' AND jsonb_typeof(metadata #> '{config,remove_default}') = 'array'
  AND metadata #> '{config,remove_default}' ?| ARRAY['schedule_create','schedule_list','schedule_update','schedule_delete']) THEN
  RAISE EXCEPTION 'registry 0064_schedule_tools cannot be reversed: Agents remove Schedule tools'
   USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_is_valid_before_schedules_0064(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid_before_schedules_0064(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_descriptor_is_valid_before_schedules_0064(jsonb);
 original := pg_get_functiondef('public.aidash_agent_before_schedules_0064(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_before_schedules_0064(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_agent_before_schedules_0064(jsonb);
END $$;
