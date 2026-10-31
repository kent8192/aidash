-- PostgreSQL function bodies have no typed Reinhardt migration operation. DDL only.
-- Preserve the current Agent/descriptor contracts, including Context Sources,
-- Agent Plugins, completion and deferred Exposure, by delegating their old rules.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_agent_before_provider_tools_0050(jsonb)') IS NOT NULL
 OR to_regprocedure('public.aidash_descriptor_before_provider_tools_0050(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'Provider tool contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_bindings_is_valid(', 'FUNCTION public.aidash_agent_before_provider_tools_0050(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract function header missing'; END IF;
 EXECUTE copied;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_before_provider_tools_0050(');
 IF copied = original THEN RAISE EXCEPTION 'Descriptor contract function header missing'; END IF;
 EXECUTE copied;
END $$;

-- This is the closed v1 Binding union: defaults are authored explicitly and no
-- engine/panel default can silently expand spending. Exact tool-type matching
-- and immutable parameter digests remain the application/broker boundary.
CREATE OR REPLACE FUNCTION public.aidash_provider_tool_binding_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE parameters jsonb; key text;
BEGIN
 IF NOT public.jsonb_matches_schema($provider_tools${"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"required":["max_tool_calls","parameters"],"properties":{"max_tool_calls":{"type":"integer","minimum":1,"maximum":8},"parameters":{"oneOf":[{"type":"object","additionalProperties":false,"required":["engine","max_results","max_total_results"],"properties":{"engine":{"enum":["exa","parallel","perplexity"]},"max_results":{"type":"integer","minimum":1,"maximum":10},"max_total_results":{"type":"integer","minimum":1,"maximum":30},"allowed_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}},"excluded_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}}},"not":{"required":["allowed_domains","excluded_domains"]}},{"type":"object","additionalProperties":false,"required":["engine","max_content_tokens"],"properties":{"engine":{"enum":["openrouter","exa","parallel"]},"max_content_tokens":{"type":"integer","minimum":1,"maximum":16384},"allowed_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}},"blocked_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}}}},{"type":"object","additionalProperties":false,"required":["timezone"],"properties":{"timezone":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[-A-Za-z0-9/_+]+$"}}},{"type":"object","additionalProperties":false,"required":["analysis_models","model","max_tool_calls","max_completion_tokens"],"properties":{"analysis_models":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}}},"model":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}},"max_tool_calls":{"type":"integer","minimum":1,"maximum":4},"max_completion_tokens":{"type":"integer","minimum":1,"maximum":8192}}}]}}}$provider_tools$::json, value) THEN RETURN false; END IF;
 parameters := value->'parameters';
 IF NOT (value->>'max_tool_calls') ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 FOREACH key IN ARRAY ARRAY['max_results','max_total_results','max_content_tokens','max_tool_calls','max_completion_tokens'] LOOP
  IF parameters ? key AND NOT (parameters->>key) ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 END LOOP;
 IF parameters ? 'max_results' AND (parameters->>'max_total_results')::numeric < (parameters->>'max_results')::numeric THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE item jsonb; restriction jsonb; edges jsonb := '[]'::jsonb;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR jsonb_typeof(COALESCE(value->'bindings','[]'::jsonb)) IS DISTINCT FROM 'array' THEN RETURN false; END IF;
 FOR item IN SELECT jsonb_array_elements(COALESCE(value->'bindings','[]'::jsonb)) LOOP
  restriction := COALESCE(item->'narrow','{}'::jsonb);
  IF jsonb_typeof(item) IS DISTINCT FROM 'object' OR jsonb_typeof(restriction) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
  IF COALESCE(restriction->'provider_tool','null'::jsonb) <> 'null'::jsonb
  AND NOT COALESCE(item->>'kind' IN ('tool','bundle') AND public.aidash_provider_tool_binding_is_valid(restriction->'provider_tool'),false) THEN RETURN false; END IF;
  item := jsonb_set(item, '{narrow}', restriction - 'provider_tool');
  edges := edges || jsonb_build_array(item);
 END LOOP;
 RETURN public.aidash_agent_before_provider_tools_0050(jsonb_set(value, '{bindings}', edges));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE restriction jsonb; operation text;
BEGIN
 restriction := COALESCE(value->'narrow','{}'::jsonb);
 IF jsonb_typeof(restriction) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 -- Definition parameters are not an author-controlled Binding. Only the
 -- Deployment's canonical, immutable descriptor can claim the Provider tier.
 IF COALESCE(restriction->'provider_tool','null'::jsonb) <> 'null'::jsonb THEN RETURN false; END IF;
 restriction := restriction - 'provider_tool';
 IF value->>'tier' = 'provider' OR value->>'provider' LIKE 'provider.openrouter.%' OR value->>'provider' LIKE 'openrouter:%' THEN
  operation := CASE value->>'provider'
   WHEN 'provider.openrouter.web_search@1' THEN 'provider_web_search' WHEN 'openrouter:web_search' THEN 'provider_web_search'
   WHEN 'provider.openrouter.web_fetch@1' THEN 'provider_web_fetch' WHEN 'openrouter:web_fetch' THEN 'provider_web_fetch'
   WHEN 'provider.openrouter.datetime@1' THEN 'provider_datetime' WHEN 'openrouter:datetime' THEN 'provider_datetime'
   WHEN 'provider.openrouter.fusion@1' THEN 'provider_fusion' WHEN 'openrouter:fusion' THEN 'provider_fusion' END;
  RETURN COALESCE(jsonb_typeof(value) = 'object'
   AND value - ARRAY['registry_deployment','provider','operation','default_alias','tier','narrow','transport','lifecycle']::text[] = '{}'::jsonb
   AND value->>'registry_deployment' ~ '^aidash://[A-Za-z0-9-]{1,100}$'
   AND operation IS NOT NULL AND value->>'operation' = operation AND value->>'default_alias' = operation
   AND value->>'tier' = 'provider' AND restriction = '{}'::jsonb
   AND COALESCE(value->'transport','null'::jsonb) = 'null'::jsonb
   AND COALESCE(value->'lifecycle','null'::jsonb) = 'null'::jsonb,false);
 END IF;
 RETURN public.aidash_descriptor_before_provider_tools_0050(jsonb_set(value, '{narrow}', restriction));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
