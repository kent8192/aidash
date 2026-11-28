-- PostgreSQL validation function bodies require SQL DDL.
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_before_drive_0179(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_drive_0179(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE equivalent text;
BEGIN
 IF value->>'provider' IS DISTINCT FROM 'core.drives@1' THEN
  RETURN public.aidash_descriptor_before_drive_0179(value);
 END IF;
 equivalent := CASE value->>'operation'
  WHEN 'drive_publish' THEN 'apply_patch'
  WHEN 'drive_publish_status' THEN 'apply_patch'
  WHEN 'drive_search' THEN 'file_search' END;
 IF equivalent IS NULL OR value->>'default_alias' IS DISTINCT FROM value->>'operation' THEN
  RETURN false;
 END IF;
 RETURN public.aidash_descriptor_before_drive_0179(value || jsonb_build_object(
  'provider', 'core.files@1', 'operation', equivalent, 'default_alias', equivalent));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE removals jsonb;
BEGIN
 removals := value->'remove_default';
 IF jsonb_typeof(removals) = 'array' AND removals ? 'drive_search' THEN
  IF (SELECT count(*) FROM jsonb_array_elements(removals) item WHERE item = '"drive_search"'::jsonb) > 1 THEN
   RETURN false;
  END IF;
  -- Recognize the extra default without collapsing it into the file_search default.
  value := jsonb_set(value, '{remove_default}', removals - 'drive_search');
 END IF;
 RETURN public.aidash_agent_before_drive_0179(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
