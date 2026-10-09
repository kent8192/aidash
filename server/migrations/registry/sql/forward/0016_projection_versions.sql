-- PostgreSQL function bodies and JSONB CHECK expressions have no typed migration operation. DDL only.
-- Agents may name a Projection Version and models may declare the versions they accept (ADR 0015).
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE item jsonb; removals jsonb; edges jsonb; step_count numeric; restriction jsonb; candidate jsonb;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR value - ARRAY['schema_version','model','instructions','bindings','remove_default','cluster','max_steps','projection_version']::text[] <> '{}'::jsonb
 OR NOT COALESCE(value->'projection_version','null'::jsonb) IN ('null'::jsonb,'"legacy"'::jsonb,'"ordered"'::jsonb,'"native"'::jsonb)
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb
 OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_node":"aidash://contract"}'::jsonb),false)
 OR jsonb_typeof(COALESCE(value->'instructions','""'::jsonb)) <> 'string' THEN RETURN false; END IF;
 IF COALESCE(value->'cluster','null'::jsonb) <> 'null'::jsonb
 AND NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'cluster' || '{"registry_node":"aidash://contract"}'::jsonb),false) THEN RETURN false; END IF;
 IF jsonb_typeof(COALESCE(value->'max_steps','64'::jsonb)) <> 'number' OR NOT COALESCE(COALESCE(value->>'max_steps','64') ~ '^[0-9]+$',false) THEN RETURN false; END IF;
 step_count := COALESCE(value->>'max_steps','64')::numeric;
 IF step_count NOT BETWEEN 1 AND 1000 THEN RETURN false; END IF;
 edges := COALESCE(value->'bindings','[]'::jsonb);
 removals := COALESCE(value->'remove_default','[]'::jsonb);
 IF jsonb_typeof(edges) <> 'array' OR jsonb_typeof(removals) <> 'array' THEN RETURN false; END IF;
 IF jsonb_array_length(edges) > 128 THEN RETURN false; END IF;
 FOR item IN SELECT jsonb_array_elements(edges) LOOP
  IF jsonb_typeof(item) <> 'object' OR item - ARRAY['kind','target','alias','narrow','members']::text[] <> '{}'::jsonb
  OR NOT COALESCE(item->>'kind' IN ('tool','bundle','skill','memory','source'),false)
  OR NOT COALESCE(public.aidash_qualified_ref_is_valid(item->'target'),false)
  OR jsonb_typeof(COALESCE(item->'narrow','{}'::jsonb)) <> 'object'
  OR COALESCE(item->'narrow','{}'::jsonb) - ARRAY['allowed_hosts','scope','limits']::text[] <> '{}'::jsonb THEN RETURN false; END IF;
  restriction := COALESCE(item->'narrow','{}'::jsonb);
  IF COALESCE(restriction->'allowed_hosts','null'::jsonb) <> 'null'::jsonb THEN
   IF jsonb_typeof(restriction->'allowed_hosts') <> 'array' OR jsonb_array_length(restriction->'allowed_hosts') = 0
   OR jsonb_path_exists(restriction->'allowed_hosts','strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
  END IF;
  IF jsonb_typeof(COALESCE(restriction->'scope','{}'::jsonb)) <> 'object' OR jsonb_typeof(COALESCE(restriction->'limits','{}'::jsonb)) <> 'object' THEN RETURN false; END IF;
  FOR candidate IN SELECT v FROM jsonb_each(COALESCE(restriction->'scope','{}'::jsonb)) AS fields(k,v) LOOP
   IF jsonb_typeof(candidate) <> 'array' OR jsonb_array_length(candidate) = 0 OR jsonb_path_exists(candidate,'strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
  END LOOP;
  FOR candidate IN SELECT v FROM jsonb_each(COALESCE(restriction->'limits','{}'::jsonb)) AS fields(k,v) LOOP
   IF jsonb_typeof(candidate) <> 'number' OR NOT candidate::text ~ '^[1-9][0-9]*$' OR candidate::text::numeric > 18446744073709551615 THEN RETURN false; END IF;
  END LOOP;
  IF COALESCE(item->'alias','null'::jsonb) <> 'null'::jsonb
  AND (jsonb_typeof(item->'alias') <> 'string' OR item->>'kind' <> 'tool' OR NOT COALESCE(item->>'alias' ~ '^[A-Za-z0-9_-]{1,64}$',false)) THEN RETURN false; END IF;
  IF COALESCE(item->'members','null'::jsonb) <> 'null'::jsonb THEN
   IF item->>'kind' <> 'bundle' OR jsonb_typeof(item->'members') <> 'array'
   OR jsonb_array_length(item->'members') > 128
   OR jsonb_path_exists(item->'members','strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
   IF (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements(item->'members') e) THEN RETURN false; END IF;
  END IF;
 END LOOP;
 IF (SELECT count(*) <> count(DISTINCT e->'target') FROM jsonb_array_elements(edges) e) THEN RETURN false; END IF;
 FOR item IN SELECT jsonb_array_elements(removals) LOOP
  IF jsonb_typeof(item) <> 'string' OR NOT (item #>> '{}') = ANY(ARRAY['workspace_observe','workspace_wait','skill_list','skill_load','skill_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','memory_mutate','memory_recall','memory_reflect']) THEN RETURN false; END IF;
 END LOOP;
 IF (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements(removals) e) THEN RETURN false; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(edges) e WHERE e->>'kind' = 'skill')
 AND removals ?| ARRAY['skill_list','skill_load','skill_read'] THEN RETURN false; END IF;
 RETURN length(btrim(COALESCE(value->>'instructions',''), U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0
 OR EXISTS(SELECT 1 FROM jsonb_array_elements(edges) e WHERE e->>'kind' IN ('skill','source'));
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;

ALTER TABLE registry DROP CONSTRAINT registry_model_config;
ALTER TABLE registry ADD CONSTRAINT registry_model_config CHECK (COALESCE((((kind <> 'model'::text) OR (((metadata #>> '{config,provider}'::text[]) = 'openrouter'::text) AND (jsonb_typeof((metadata #> '{config,model_id}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,model_id}'::text[]), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND public.aidash_valid_http_endpoint((metadata #> '{config,endpoint}'::text[])) AND
						CASE
						WHEN (jsonb_typeof((metadata #> '{config,context_window}'::text[])) = 'number'::text) THEN ((((metadata #>> '{config,context_window}'::text[]))::numeric >= (2048)::numeric) AND (trunc(((metadata #>> '{config,context_window}'::text[]))::numeric) = ((metadata #>> '{config,context_window}'::text[]))::numeric))
						ELSE false
						END AND (jsonb_typeof((metadata #> '{config,modalities}'::text[])) = 'array'::text) AND ((metadata #> '{config,modalities}'::text[]) @> '["text"]'::jsonb) AND ((NOT ((metadata -> 'config'::text) ? 'reasoning_effort'::text)) OR ((metadata #> '{config,reasoning_effort}'::text[]) = 'null'::jsonb) OR ((metadata #>> '{config,reasoning_effort}'::text[]) = ANY (ARRAY['none'::text, 'minimal'::text, 'low'::text, 'medium'::text, 'high'::text, 'xhigh'::text, 'max'::text]))))) AND ((kind <> 'model'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'model_id'::text, 'endpoint'::text, 'credential_env'::text, 'reasoning_effort'::text, 'context_window'::text, 'max_output_tokens'::text, 'modalities'::text, 'cost'::text, 'request_timeout_secs'::text, 'media_routes'::text, 'projection_versions'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((metadata -> 'config'::text) ? 'cost'::text) AND (jsonb_typeof(COALESCE((metadata #> '{config,credential_env}'::text[]), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND (jsonb_typeof((metadata #> '{config,modalities}'::text[])) = 'array'::text) AND (NOT jsonb_path_exists((metadata #> '{config,modalities}'::text[]), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND
						CASE
						WHEN ((jsonb_typeof((metadata #> '{config,context_window}'::text[])) = 'number'::text) AND (((metadata #> '{config,context_window}'::text[]))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((metadata #> '{config,context_window}'::text[]))::text)::numeric <= '18446744073709551615'::numeric)
						ELSE false
						END AND
						CASE
						WHEN ((NOT ((metadata -> 'config'::text) ? 'max_output_tokens'::text)) OR (((metadata -> 'config'::text) -> 'max_output_tokens'::text) = 'null'::jsonb)) THEN true
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_output_tokens'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'max_output_tokens'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'context_window'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'context_window'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((metadata -> 'config'::text) ->> 'max_output_tokens'::text))::numeric >= (1)::numeric) AND ((((metadata -> 'config'::text) ->> 'max_output_tokens'::text))::numeric <= LEAST(('4294967295'::bigint)::numeric, (((metadata -> 'config'::text) ->> 'context_window'::text))::numeric)))
						ELSE false
						END
						ELSE false
						END)) AND ((kind <> 'model'::text) OR
						CASE
						WHEN (((metadata #> '{config,request_timeout_secs}'::text[]) IS NULL) OR ((metadata #> '{config,request_timeout_secs}'::text[]) = 'null'::jsonb)) THEN true
						WHEN ((jsonb_typeof((metadata #> '{config,request_timeout_secs}'::text[])) = 'number'::text) AND (((metadata #> '{config,request_timeout_secs}'::text[]))::text ~ '^[1-9][0-9]*$'::text)) THEN (((((metadata #> '{config,request_timeout_secs}'::text[]))::text)::numeric >= (1)::numeric) AND ((((metadata #> '{config,request_timeout_secs}'::text[]))::text)::numeric <= ('4294967295'::bigint)::numeric))
						ELSE false
						END) AND ((kind <> 'model'::text) OR ((NOT ((metadata -> 'config'::text) ? 'media_routes'::text)) OR public.aidash_media_routes_valid((metadata #> '{config,media_routes}'::text[])))) AND ((kind <> 'model'::text) OR (NOT ((metadata -> 'config'::text) ? 'projection_versions'::text)) OR ((jsonb_typeof((metadata #> '{config,projection_versions}'::text[])) = 'array'::text) AND ('["legacy", "ordered", "native"]'::jsonb @> (metadata #> '{config,projection_versions}'::text[]))))), false));
