-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Extend the closed Provider union; retain all prior validation functions and OIDs.
DO $$
DECLARE original text; copied text; name text; backup text;
BEGIN
 FOREACH name IN ARRAY ARRAY['aidash_provider_tool_binding_is_valid','aidash_descriptor_is_valid'] LOOP
  backup := name || '_before_warning_0051';
  IF to_regprocedure('public.' || backup || '(jsonb)') IS NOT NULL THEN RAISE EXCEPTION 'warning contract backup already exists'; END IF;
  original := pg_get_functiondef(('public.' || name || '(jsonb)')::regprocedure);
  copied := replace(original, 'FUNCTION public.' || name || '(', 'FUNCTION public.' || backup || '(');
  IF copied = original THEN RAISE EXCEPTION 'contract header missing'; END IF;
  EXECUTE copied;
 END LOOP;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_provider_tool_binding_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE parameters jsonb; key text;
BEGIN
 IF public.aidash_provider_tool_binding_is_valid_before_warning_0051(value) THEN RETURN true; END IF;
 IF NOT public.jsonb_matches_schema($warning_tools${"type":"object","additionalProperties":false,"required":["max_tool_calls","parameters"],"properties":{"max_tool_calls":{"type":"integer","minimum":1,"maximum":8},"parameters":{"oneOf":[{"type":"object","additionalProperties":false,"required":["model","max_completion_tokens","forward_transcript","stream"],"properties":{"model":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}},"max_completion_tokens":{"type":"integer","minimum":1,"maximum":8192},"forward_transcript":{"type":"boolean"},"stream":{"const":false},"instructions":{"type":"string","maxLength":8192}}},{"type":"object","additionalProperties":false,"required":["model","max_completion_tokens","max_tool_calls","tools","inherit_functions"],"properties":{"model":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}},"max_completion_tokens":{"type":"integer","minimum":1,"maximum":8192},"max_tool_calls":{"type":"integer","minimum":1,"maximum":4},"tools":{"const":[]},"instructions":{"type":"string","maxLength":8192},"inherit_functions":{"const":false}}},{"type":"object","additionalProperties":false,"required":["max_results"],"properties":{"max_results":{"type":"integer","minimum":1,"maximum":10}}},{"type":"object","additionalProperties":false,"required":["engine","environment"],"properties":{"engine":{"const":"openrouter"},"environment":{"type":"object","additionalProperties":false,"required":["type","network_policy"],"properties":{"type":{"const":"container_reference"},"network_policy":{"oneOf":[{"type":"object","additionalProperties":false,"required":["type"],"properties":{"type":{"const":"disabled"}}},{"type":"object","additionalProperties":false,"required":["type","allowed_domains"],"properties":{"type":{"const":"allowlist"},"allowed_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$"}}}}]}}}}}]}},"$schema":"https://json-schema.org/draft/2020-12/schema"}$warning_tools$::json, value) THEN RETURN false; END IF;
 IF NOT (value->>'max_tool_calls') ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 parameters := value->'parameters';
 FOREACH key IN ARRAY ARRAY['max_results','max_completion_tokens','max_tool_calls'] LOOP
  IF parameters ? key AND NOT (parameters->>key) ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 END LOOP;
 RETURN true;
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE operation text;
BEGIN
 operation := CASE value->>'provider'
 WHEN 'provider.openrouter.advisor@1' THEN 'provider_advisor' WHEN 'openrouter:advisor' THEN 'provider_advisor'
 WHEN 'provider.openrouter.subagent@1' THEN 'provider_subagent' WHEN 'openrouter:subagent' THEN 'provider_subagent'
 WHEN 'provider.openrouter.experimental__search_models@1' THEN 'provider_search_models' WHEN 'openrouter:experimental__search_models' THEN 'provider_search_models'
 WHEN 'provider.openrouter.tool_search@1' THEN 'provider_tool_search' WHEN 'openrouter:tool_search' THEN 'provider_tool_search'
 WHEN 'provider.openrouter.shell@1' THEN 'provider_shell' WHEN 'openrouter:shell' THEN 'provider_shell'
 END;
 IF operation IS NULL THEN RETURN public.aidash_descriptor_is_valid_before_warning_0051(value); END IF;
 IF value->>'operation' IS DISTINCT FROM operation OR value->>'default_alias' IS DISTINCT FROM operation THEN RETURN false; END IF;
 -- Reuse the established descriptor shape/tier/no-transport rules, without relaxing them.
 RETURN public.aidash_descriptor_is_valid_before_warning_0051(value || '{"provider":"provider.openrouter.datetime@1","operation":"provider_datetime","default_alias":"provider_datetime"}'::jsonb);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
