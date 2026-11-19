-- Validation function DDL; do not invalidate stored Thread tool descriptors or Agents.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' = 'core.threads@1') THEN
  RAISE EXCEPTION 'registry 0081_thread_tools cannot be reversed: definitions declare Thread tools'
   USING ERRCODE = '55000';
 END IF;
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'agent' AND jsonb_typeof(metadata #> '{config,remove_default}') = 'array'
  AND metadata #> '{config,remove_default}' ?| ARRAY['thread_observe','thread_search','thread_message','task_wait']) THEN
  RAISE EXCEPTION 'registry 0081_thread_tools cannot be reversed: Agents remove Thread tools'
   USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_is_valid_before_threads_0081(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid_before_threads_0081(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_descriptor_is_valid_before_threads_0081(jsonb);
 original := pg_get_functiondef('public.aidash_agent_before_threads_0081(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_before_threads_0081(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_agent_before_threads_0081(jsonb);
END $$;
