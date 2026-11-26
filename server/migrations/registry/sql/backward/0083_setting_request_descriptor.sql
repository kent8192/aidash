-- Restore the 0081 descriptor only if existing rows remain valid.
DO $$
BEGIN
 IF EXISTS (SELECT 1 FROM registry WHERE kind = 'tool'
  AND metadata #>> '{config,operation}' = 'setting_request') THEN
  RAISE EXCEPTION 'registry 0083 cannot reverse while setting_request descriptors exist'
   USING ERRCODE = '55000';
 END IF;
END $$;
CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE former text;
BEGIN
 IF value->>'provider' IS DISTINCT FROM 'core.threads@1' THEN RETURN public.aidash_descriptor_is_valid_before_threads_0081(value); END IF;
 former := CASE value->>'operation'
  WHEN 'thread_read' THEN 'workspace_read' WHEN 'thread_observe' THEN 'workspace_observe'
  WHEN 'thread_search' THEN 'workspace_search' WHEN 'thread_message' THEN 'workspace_message'
  WHEN 'task_wait' THEN 'workspace_wait' END;
 IF former IS NULL OR value->>'default_alias' IS DISTINCT FROM value->>'operation' THEN RETURN false; END IF;
 RETURN public.aidash_descriptor_is_valid_before_threads_0081(value || jsonb_build_object(
  'provider', 'core.workspace@1', 'operation', former, 'default_alias', former));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
