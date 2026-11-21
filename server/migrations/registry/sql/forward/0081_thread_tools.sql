-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit the core.threads@1 builtins (#1008, ADR 0140) thread_read,
-- thread_observe, thread_search, thread_message and task_wait, which replace
-- the core.workspace@1 tools as default tools, with exactly the rules of the
-- tools they replace, and let an Agent remove them from its default tools;
-- retain the previous validators.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_threads_0081(jsonb)') IS NOT NULL
 OR to_regprocedure('public.aidash_agent_before_threads_0081(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'thread tool contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_threads_0081(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_threads_0081(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract header missing'; END IF;
 EXECUTE copied;
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

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF value ? 'remove_default' AND jsonb_typeof(value->'remove_default') = 'array' THEN
  -- The Thread default tools are removed like the Workspace tools they replace.
  value := jsonb_set(value, '{remove_default}', COALESCE((
   SELECT jsonb_agg(CASE
     WHEN jsonb_typeof(e) = 'string' AND e #>> '{}' = 'thread_observe' THEN to_jsonb('workspace_observe'::text)
     WHEN jsonb_typeof(e) = 'string' AND e #>> '{}' = 'thread_search' THEN to_jsonb('workspace_search'::text)
     WHEN jsonb_typeof(e) = 'string' AND e #>> '{}' = 'thread_message' THEN to_jsonb('workspace_message'::text)
     WHEN jsonb_typeof(e) = 'string' AND e #>> '{}' = 'task_wait' THEN to_jsonb('workspace_wait'::text)
     ELSE e END ORDER BY o)
   FROM jsonb_array_elements(value->'remove_default') WITH ORDINALITY AS r(e, o)), '[]'::jsonb));
 END IF;
 RETURN public.aidash_agent_before_threads_0081(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
