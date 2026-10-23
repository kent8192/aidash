-- PostgreSQL function bodies have no typed Reinhardt expression. DDL only.
CREATE OR REPLACE FUNCTION public.aidash_source_declaration_is_valid(value jsonb, operation text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE tools jsonb; pin jsonb; disclosure jsonb; candidate jsonb;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['provider','operation','tools','pin','volatility','query','refresh','disclosure','required_home']::text[] <> '{}'::jsonb
 OR value->>'provider' IS DISTINCT FROM 'aidash.native-context/1' OR value->>'operation' IS DISTINCT FROM operation THEN RETURN false; END IF;
 tools := value->'tools';
 IF jsonb_typeof(tools) IS DISTINCT FROM 'object' OR tools - ARRAY['read','write','detail']::text[] <> '{}'::jsonb
 OR jsonb_typeof(tools->'read') IS DISTINCT FROM 'array' OR jsonb_typeof(tools->'write') IS DISTINCT FROM 'array'
 OR jsonb_array_length(tools->'read')+jsonb_array_length(tools->'write') > 32 THEN RETURN false; END IF;
 FOR candidate IN SELECT jsonb_array_elements((tools->'read') || (tools->'write')) LOOP
  IF NOT COALESCE(public.aidash_qualified_ref_is_valid(candidate),false) THEN RETURN false; END IF;
 END LOOP;
 IF (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements((tools->'read') || (tools->'write')) e) THEN RETURN false; END IF;
 IF COALESCE(tools->'detail','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_qualified_ref_is_valid(tools->'detail'),false) THEN RETURN false; END IF;
 IF value ? 'volatility' AND value->>'volatility' NOT IN ('run','step') THEN RETURN false; END IF;
 IF value ? 'query' AND value->>'query' NOT IN ('task-inputs@1','accepted-inputs-first@1') THEN RETURN false; END IF;
 IF value ? 'refresh' AND value->>'refresh' IS DISTINCT FROM 'revision-bound@1' THEN RETURN false; END IF;
 IF value ? 'required_home' AND jsonb_typeof(value->'required_home') <> 'boolean' THEN RETURN false; END IF;
 disclosure := COALESCE(value->'disclosure','{"previews":false,"query":false}'::jsonb);
 IF jsonb_typeof(disclosure) <> 'object' OR disclosure - ARRAY['previews','query']::text[] <> '{}'::jsonb
 OR jsonb_typeof(disclosure->'previews') IS DISTINCT FROM 'boolean' OR jsonb_typeof(disclosure->'query') IS DISTINCT FROM 'boolean' THEN RETURN false; END IF;
 pin := COALESCE(value->'pin','{"policy":"always"}'::jsonb);
 IF jsonb_typeof(pin) <> 'object' THEN RETURN false; END IF;
 IF pin->>'policy' IN ('always','on_demand') THEN RETURN pin - 'policy' = '{}'::jsonb; END IF;
 RETURN pin->>'policy' = 'scored' AND pin - ARRAY['policy','mode','decider']::text[] = '{}'::jsonb
 AND pin->>'mode' IN ('shadow','enforce') AND COALESCE(public.aidash_qualified_ref_is_valid(pin->'decider'),false)
 AND disclosure->'previews' = 'true'::jsonb AND disclosure->'query' = 'true'::jsonb
 AND NOT (COALESCE(value->'required_home','false'::jsonb) = 'true'::jsonb AND pin->>'mode' = 'enforce');
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_context_is_valid(value jsonb, kind text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE source jsonb; adapter text;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['schema_version','source','context_source']::text[] <> '{}'::jsonb
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb OR jsonb_typeof(value->'source') <> 'object' THEN RETURN false; END IF;
 source := value->'source'; adapter := source->>'adapter';
 IF value ? 'context_source' AND NOT COALESCE(public.aidash_source_declaration_is_valid(value->'context_source', CASE adapter WHEN 'private_references' THEN 'references' WHEN 'reference_attachments' THEN 'references' WHEN 'skill_attachments' THEN 'skills' WHEN 'skill_roots' THEN 'skills' ELSE adapter END),false) THEN RETURN false; END IF;
 IF kind = 'memory' THEN RETURN adapter IN ('conversation_memory','semantic_memory') AND source - 'adapter' = '{}'::jsonb; END IF;
 IF kind <> 'source' THEN RETURN false; END IF;
 RETURN CASE adapter
 WHEN 'workspace_observation' THEN source - 'adapter' = '{}'::jsonb
 WHEN 'workspace_retrieval' THEN source - ARRAY['adapter','on_unavailable']::text[] = '{}'::jsonb
  AND (NOT source ? 'on_unavailable' OR (jsonb_typeof(source->'on_unavailable') = 'string' AND source->>'on_unavailable' IN ('pause','degrade')))
 WHEN 'private_references' THEN source - ARRAY['adapter','digest']::text[] = '{}'::jsonb AND source->>'digest' ~ '^[a-fA-F0-9]{64}$'
 WHEN 'reference_attachments' THEN source - ARRAY['adapter','references']::text[] = '{}'::jsonb AND jsonb_typeof(source->'references') = 'array' AND jsonb_array_length(source->'references') BETWEEN 1 AND 8
 WHEN 'skill_attachments' THEN source - ARRAY['adapter','attachments']::text[] = '{}'::jsonb AND jsonb_typeof(source->'attachments') = 'array' AND jsonb_array_length(source->'attachments') BETWEEN 1 AND 16
 WHEN 'skill_roots' THEN source - ARRAY['adapter','roots']::text[] = '{}'::jsonb AND jsonb_typeof(source->'roots') = 'array' AND jsonb_array_length(source->'roots') BETWEEN 1 AND 8 AND NOT jsonb_path_exists(source->'roots','strict $[*] ? (@.type() != "string")')
 ELSE false END;
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;

CREATE OR REPLACE FUNCTION public.aidash_native_context_before_sources(value jsonb, kind text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE policy jsonb; bounds jsonb;
BEGIN
 IF kind = 'memory' THEN
  IF NOT public.jsonb_matches_schema($native_memory${"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/$defs/ProviderConfig","$defs":{"ProviderConfig":{"type":"object","properties":{"engine":{"type":"string","enum":["hindsight_rust"]},"policy":{"$ref":"#/$defs/Policy"}},"required":["engine","policy"],"additionalProperties":false},"SourceConfig":{"type":"object","properties":{"scope":{"type":"string","enum":["participant","workspace"]},"memory":{"$ref":"#/$defs/EntityRef"},"max_tokens":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["scope","memory","max_tokens"],"additionalProperties":false},"Policy":{"type":"object","properties":{"extraction":{"$ref":"#/$defs/EntityRef"},"derivation":{"$ref":"#/$defs/EntityRef"},"reflection":{"$ref":"#/$defs/EntityRef"},"embedding":{"$ref":"#/$defs/EntityRef"},"reranker":{"$ref":"#/$defs/EntityRef"},"tokenizer":{"$ref":"#/$defs/EntityRef"},"semantic_link_min_similarity_millionths":{"type":"integer","minimum":1,"maximum":1000000},"prices":{"$ref":"#/$defs/Prices"},"retention":{"$ref":"#/$defs/Retention"},"bounds":{"$ref":"#/$defs/Bounds"},"learn_from_runs":{"type":"boolean"},"maintain_observations":{"type":"boolean"},"refresh_mental_models":{"type":"boolean"}},"required":["extraction","derivation","reflection","embedding","reranker","tokenizer","semantic_link_min_similarity_millionths","prices","retention","bounds","learn_from_runs","maintain_observations","refresh_mental_models"],"additionalProperties":false},"Retention":{"type":"object","properties":{"unit_max_age_days":{"type":["integer","null"],"minimum":1,"maximum":3650},"candidate_days":{"type":"integer","minimum":1,"maximum":3650},"history_days":{"type":"integer","minimum":1,"maximum":3650},"history_versions":{"type":"integer","minimum":1,"maximum":1024},"model_result_days":{"type":"integer","minimum":1,"maximum":3650},"backup_days":{"type":"integer","minimum":1,"maximum":3650},"purge_after_seconds":{"type":"integer","minimum":1,"maximum":86400},"purge_batch":{"type":"integer","minimum":1,"maximum":1024},"max_unit_records":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_operations":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["candidate_days","history_days","history_versions","model_result_days","backup_days","purge_after_seconds","purge_batch","max_unit_records","max_model_operations"],"additionalProperties":false},"Prices":{"type":"object","properties":{"extraction":{"$ref":"#/$defs/Rate"},"derivation":{"$ref":"#/$defs/Rate"},"reflection":{"$ref":"#/$defs/Rate"},"embedding":{"$ref":"#/$defs/Rate"},"reranker":{"$ref":"#/$defs/Rate"}},"required":["extraction","derivation","reflection","embedding","reranker"],"additionalProperties":false},"Rate":{"type":"object","properties":{"input_per_million":{"type":"integer","minimum":0,"maximum":18446744073709551615},"output_per_million":{"type":"integer","minimum":0,"maximum":18446744073709551615}},"required":["input_per_million","output_per_million"],"additionalProperties":false},"Bounds":{"type":"object","properties":{"max_unit_bytes":{"type":"integer","minimum":1,"maximum":2147483647},"max_input_bytes":{"type":"integer","minimum":1,"maximum":2147483647},"max_units":{"type":"integer","minimum":1,"maximum":2147483647},"max_candidates":{"type":"integer","minimum":1,"maximum":2147483647},"max_entities":{"type":"integer","minimum":1,"maximum":2147483647},"max_evidence":{"type":"integer","minimum":1,"maximum":2147483647},"max_links":{"type":"integer","minimum":1,"maximum":2147483647},"max_graph_hops":{"type":"integer","minimum":1,"maximum":2147483647},"max_graph_visits":{"type":"integer","minimum":1,"maximum":2147483647},"max_results":{"type":"integer","minimum":1,"maximum":2147483647},"max_context_tokens":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_calls":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_tokens":{"type":"integer","minimum":1,"maximum":2147483647},"max_cost_micros":{"type":"integer","minimum":1,"maximum":9223372036854775807},"max_retries":{"type":"integer","minimum":1,"maximum":2147483647},"max_call_seconds":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["max_unit_bytes","max_input_bytes","max_units","max_candidates","max_entities","max_evidence","max_links","max_graph_hops","max_graph_visits","max_results","max_context_tokens","max_model_calls","max_model_tokens","max_cost_micros","max_retries","max_call_seconds"],"additionalProperties":false},"EntityRef":{"type":"object","required":["id","version"],"properties":{"id":{"type":"string","minLength":1},"version":{"type":"string","pattern":"^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:[-+].+)?$"}},"additionalProperties":false}}}$native_memory$::json,value) THEN RETURN false; END IF;
  policy := value->'policy'; bounds := policy->'bounds';
  RETURN (bounds->>'max_candidates')::numeric <= (bounds->>'max_units')::numeric
    AND (bounds->>'max_results')::numeric <= (bounds->>'max_candidates')::numeric
    AND (policy #>> '{retention,max_unit_records}')::numeric >= (bounds->>'max_units')::numeric;
 ELSIF kind = 'source' THEN
  IF NOT public.jsonb_matches_schema($native_source${"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/$defs/SourceConfig","$defs":{"SourceConfig":{"type":"object","properties":{"scope":{"type":"string","enum":["participant","workspace"]},"memory":{"$ref":"#/$defs/EntityRef"},"max_tokens":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["scope","memory","max_tokens"],"additionalProperties":false},"EntityRef":{"type":"object","required":["id","version"],"properties":{"id":{"type":"string","minLength":1},"version":{"type":"string","pattern":"^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:[-+].+)?$"}},"additionalProperties":false}}}$native_source$::json,value) THEN RETURN false; END IF;
  RETURN true;
 ELSE RETURN false; END IF;
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;

CREATE OR REPLACE FUNCTION public.aidash_native_context_is_valid(value jsonb, kind text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value ? 'context_source' AND (kind <> 'source' OR NOT COALESCE(public.aidash_source_declaration_is_valid(value->'context_source','semantic_memory'),false)) THEN RETURN false; END IF;
 RETURN public.aidash_native_context_before_sources(value - 'context_source',kind);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
