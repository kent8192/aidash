-- A Setting Request has the same descriptor contract as a Human Request.
CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE former text;
BEGIN
 IF value->>'operation' = 'setting_request' THEN
  IF value->>'provider' IS DISTINCT FROM 'core.human@1' OR value->>'default_alias' IS DISTINCT FROM 'setting_request' THEN RETURN false; END IF;
  RETURN public.aidash_descriptor_is_valid_before_threads_0081(value || jsonb_build_object(
   'operation', 'human_request', 'default_alias', 'human_request'));
 END IF;
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
