-- PostgreSQL trigger function bodies require SQL DDL.
CREATE OR REPLACE FUNCTION public.sync_registry_memory_role_refs() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE config jsonb; binding jsonb; role text; expected text;
BEGIN
 IF TG_OP = 'UPDATE' THEN
  DELETE FROM registry_memory_role_refs WHERE tenant = OLD.tenant AND source_id = OLD.id AND source_version = OLD.version;
 END IF;
 config := NEW.metadata->'config';
 IF NEW.kind = 'agent' THEN
  INSERT INTO registry_memory_role_refs
   SELECT NEW.tenant, NEW.id, NEW.version, value->>'kind', ordinal::integer, value->>'kind', value#>>'{target,id}', value#>>'{target,version}'
   FROM jsonb_array_elements(COALESCE(config->'bindings','[]'::jsonb)) WITH ORDINALITY AS ref(value, ordinal)
   WHERE value->>'kind' IN ('memory','source');
 ELSIF NEW.kind = 'memory' AND config ? 'engine' THEN
  FOREACH role IN ARRAY ARRAY['extraction','derivation','reflection','embedding','reranker','tokenizer'] LOOP
   expected := CASE WHEN role IN ('extraction','derivation','reflection') THEN 'model' ELSE role END;
   binding := config->'policy'->role;
   IF jsonb_typeof(binding) = 'object' THEN
    INSERT INTO registry_memory_role_refs VALUES (NEW.tenant, NEW.id, NEW.version, role, 1, expected, binding->>'id', binding->>'version');
   ELSIF (role IN ('extraction','derivation','reflection') AND binding = '"agent_model"'::jsonb)
      OR (role = 'embedding' AND (binding IS NULL OR binding = 'null'::jsonb)) THEN
    CONTINUE;
   ELSE
    RAISE EXCEPTION 'invalid memory role reference' USING ERRCODE = '23514';
   END IF;
  END LOOP;
 ELSIF NEW.kind = 'source' AND config ? 'scope' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.tenant, NEW.id, NEW.version, 'memory', 1, 'memory', config#>>'{memory,id}', config#>>'{memory,version}');
 ELSIF NEW.kind = 'reranker' AND config->>'provider' = 'model' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.tenant, NEW.id, NEW.version, 'model', 1, 'model', config#>>'{model,id}', config#>>'{model,version}');
 END IF;
 RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_native_context_before_sources(value jsonb, kind text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE policy jsonb; bounds jsonb;
BEGIN
 IF kind = 'memory' THEN
  IF NOT public.jsonb_matches_schema($native_memory${"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/$defs/ProviderConfig","$defs":{"ProviderConfig":{"type":"object","properties":{"engine":{"type":"string","enum":["hindsight_rust"]},"policy":{"$ref":"#/$defs/Policy"}},"required":["engine","policy"],"additionalProperties":false},"SourceConfig":{"type":"object","properties":{"scope":{"type":"string","enum":["participant","workspace"]},"memory":{"$ref":"#/$defs/EntityRef"},"max_tokens":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["scope","memory","max_tokens"],"additionalProperties":false},"Policy":{"type":"object","properties":{"extraction":{"anyOf":[{"$ref":"#/$defs/EntityRef"},{"const":"agent_model"}]},"derivation":{"anyOf":[{"$ref":"#/$defs/EntityRef"},{"const":"agent_model"}]},"reflection":{"anyOf":[{"$ref":"#/$defs/EntityRef"},{"const":"agent_model"}]},"embedding":{"anyOf":[{"$ref":"#/$defs/EntityRef"},{"type":"null"}]},"reranker":{"$ref":"#/$defs/EntityRef"},"tokenizer":{"$ref":"#/$defs/EntityRef"},"semantic_link_min_similarity_millionths":{"type":"integer","minimum":1,"maximum":1000000},"prices":{"$ref":"#/$defs/Prices"},"retention":{"$ref":"#/$defs/Retention"},"bounds":{"$ref":"#/$defs/Bounds"},"learn_from_runs":{"type":"boolean"},"maintain_observations":{"type":"boolean"},"refresh_mental_models":{"type":"boolean"}},"required":["extraction","derivation","reflection","reranker","tokenizer","semantic_link_min_similarity_millionths","prices","retention","bounds","learn_from_runs","maintain_observations","refresh_mental_models"],"additionalProperties":false},"Retention":{"type":"object","properties":{"unit_max_age_days":{"type":["integer","null"],"minimum":1,"maximum":3650},"candidate_days":{"type":"integer","minimum":1,"maximum":3650},"history_days":{"type":"integer","minimum":1,"maximum":3650},"history_versions":{"type":"integer","minimum":1,"maximum":1024},"model_result_days":{"type":"integer","minimum":1,"maximum":3650},"backup_days":{"type":"integer","minimum":1,"maximum":3650},"purge_after_seconds":{"type":"integer","minimum":1,"maximum":86400},"purge_batch":{"type":"integer","minimum":1,"maximum":1024},"max_unit_records":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_operations":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["candidate_days","history_days","history_versions","model_result_days","backup_days","purge_after_seconds","purge_batch","max_unit_records","max_model_operations"],"additionalProperties":false},"Prices":{"type":"object","properties":{"extraction":{"$ref":"#/$defs/Rate"},"derivation":{"$ref":"#/$defs/Rate"},"reflection":{"$ref":"#/$defs/Rate"},"embedding":{"$ref":"#/$defs/Rate"},"reranker":{"$ref":"#/$defs/Rate"}},"required":["extraction","derivation","reflection","embedding","reranker"],"additionalProperties":false},"Rate":{"type":"object","properties":{"input_per_million":{"type":"integer","minimum":0,"maximum":18446744073709551615},"output_per_million":{"type":"integer","minimum":0,"maximum":18446744073709551615}},"required":["input_per_million","output_per_million"],"additionalProperties":false},"Bounds":{"type":"object","properties":{"max_unit_bytes":{"type":"integer","minimum":1,"maximum":2147483647},"max_input_bytes":{"type":"integer","minimum":1,"maximum":2147483647},"max_units":{"type":"integer","minimum":1,"maximum":2147483647},"max_candidates":{"type":"integer","minimum":1,"maximum":2147483647},"max_entities":{"type":"integer","minimum":1,"maximum":2147483647},"max_evidence":{"type":"integer","minimum":1,"maximum":2147483647},"max_links":{"type":"integer","minimum":1,"maximum":2147483647},"max_graph_hops":{"type":"integer","minimum":1,"maximum":2147483647},"max_graph_visits":{"type":"integer","minimum":1,"maximum":2147483647},"max_results":{"type":"integer","minimum":1,"maximum":2147483647},"max_context_tokens":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_calls":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_tokens":{"type":"integer","minimum":1,"maximum":2147483647},"max_cost_micros":{"type":"integer","minimum":1,"maximum":9223372036854775807},"max_retries":{"type":"integer","minimum":1,"maximum":2147483647},"max_call_seconds":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["max_unit_bytes","max_input_bytes","max_units","max_candidates","max_entities","max_evidence","max_links","max_graph_hops","max_graph_visits","max_results","max_context_tokens","max_model_calls","max_model_tokens","max_cost_micros","max_retries","max_call_seconds"],"additionalProperties":false},"EntityRef":{"type":"object","required":["id","version"],"properties":{"id":{"type":"string","minLength":1},"version":{"type":"string","pattern":"^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:[-+].+)?$"}},"additionalProperties":false}}}$native_memory$::json,value) THEN RETURN false; END IF;
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
