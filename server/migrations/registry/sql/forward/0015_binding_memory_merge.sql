-- PostgreSQL function bodies and NOT VALID JSON checks have no typed migration operation. DDL only.
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE item jsonb; removals jsonb; edges jsonb; step_count numeric; restriction jsonb; candidate jsonb;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR value - ARRAY['schema_version','model','instructions','bindings','remove_default','cluster','max_steps']::text[] <> '{}'::jsonb
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
CREATE OR REPLACE FUNCTION public.sync_registry_memory_role_refs() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE config jsonb; binding jsonb; role text; expected text;
BEGIN
 IF TG_OP = 'UPDATE' THEN
  DELETE FROM registry_memory_role_refs WHERE source_id = OLD.id AND source_version = OLD.version;
 END IF;
 config := NEW.metadata->'config';
 IF NEW.kind = 'agent' THEN
  INSERT INTO registry_memory_role_refs
   SELECT NEW.id, NEW.version, value->>'kind', ordinal::integer, value->>'kind', value#>>'{target,id}', value#>>'{target,version}'
   FROM jsonb_array_elements(COALESCE(config->'bindings','[]'::jsonb)) WITH ORDINALITY AS ref(value, ordinal)
   WHERE value->>'kind' IN ('memory','source');
 ELSIF NEW.kind = 'memory' AND config ? 'engine' THEN
  FOREACH role IN ARRAY ARRAY['extraction','derivation','reflection','embedding','reranker','tokenizer'] LOOP
   expected := CASE WHEN role IN ('extraction','derivation','reflection') THEN 'model' ELSE role END;
   binding := config->'policy'->role;
   INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, role, 1, expected, binding->>'id', binding->>'version');
  END LOOP;
 ELSIF NEW.kind = 'source' AND config ? 'scope' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, 'memory', 1, 'memory', config#>>'{memory,id}', config#>>'{memory,version}');
 ELSIF NEW.kind = 'reranker' AND config->>'provider' = 'model' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, 'model', 1, 'model', config#>>'{model,id}', config#>>'{model,version}');
 END IF;
 RETURN NEW;
END $$;
ALTER TABLE registry DROP CONSTRAINT registry_context_config;
ALTER TABLE registry ADD CONSTRAINT registry_context_config CHECK (kind NOT IN ('memory','source') OR NOT (metadata->'config' ? 'schema_version') OR COALESCE(public.aidash_context_is_valid(metadata->'config',kind),false)) NOT VALID;
-- PostgreSQL function bodies have no typed migration operation. DDL only.
-- Parenthesize JSON extraction before arithmetic-precedence subtraction.
CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE expected_provider text; expected_tier text; start_operation boolean;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['registry_node','provider','operation','default_alias','tier','narrow','transport','lifecycle']::text[] <> '{}'::jsonb
 OR NOT COALESCE(value->>'registry_node' ~ '^aidash://[A-Za-z0-9-]{1,100}$', false)
 OR NOT COALESCE(value->>'default_alias' ~ '^[A-Za-z0-9_-]{1,64}$', false)
 OR jsonb_typeof(COALESCE(value->'narrow','{}'::jsonb)) <> 'object' THEN RETURN false; END IF;
 IF COALESCE(value->'narrow','{}'::jsonb) - ARRAY['allowed_hosts','scope','limits']::text[] <> '{}'::jsonb THEN RETURN false; END IF;
 expected_provider := CASE value->>'operation'
  WHEN 'workspace_read' THEN 'core.workspace@1' WHEN 'workspace_observe' THEN 'core.workspace@1'
  WHEN 'workspace_wait' THEN 'core.workspace@1' WHEN 'workspace_message' THEN 'core.workspace@1'
  WHEN 'human_request' THEN 'core.human@1'
  WHEN 'task_create' THEN 'core.tasks@1' WHEN 'task_delegate' THEN 'core.tasks@1'
  WHEN 'agent_discover' THEN 'core.tasks@1' WHEN 'task_assign' THEN 'core.tasks@1'
  WHEN 'skill_list' THEN 'core.skills@1' WHEN 'skill_load' THEN 'core.skills@1' WHEN 'skill_read' THEN 'core.skills@1'
  WHEN 'file_search' THEN 'core.files@1' WHEN 'file_read' THEN 'core.files@1' WHEN 'apply_patch' THEN 'core.files@1'
  WHEN 'artifact_publish' THEN 'core.artifacts@1' WHEN 'memory_mutate' THEN 'core.memory@1' WHEN 'memory_recall' THEN 'core.memory@1' WHEN 'memory_reflect' THEN 'core.memory@1'
  WHEN 'shell' THEN 'core.sandbox@1' WHEN 'shell_poll' THEN 'core.sandbox@1' WHEN 'shell_cancel' THEN 'core.sandbox@1'
  WHEN 'code_interpreter' THEN 'core.sandbox@1' WHEN 'python_install' THEN 'core.sandbox@1'
  WHEN 'python_poll' THEN 'core.sandbox@1' WHEN 'python_cancel' THEN 'core.sandbox@1'
  WHEN 'outbound_get' THEN 'core.egress@1' WHEN 'file_share' THEN 'core.sharing@1' END;
 IF expected_provider IS NOT NULL THEN
  expected_tier := CASE WHEN value->>'operation' IN ('shell','shell_poll','shell_cancel','code_interpreter','python_install','python_poll','python_cancel','outbound_get','apply_patch','file_share','task_assign') THEN 'host' ELSE 'builtin' END;
  IF value->>'provider' IS DISTINCT FROM expected_provider OR value->>'tier' IS DISTINCT FROM expected_tier
  OR COALESCE(value->'transport','null'::jsonb) <> 'null'::jsonb THEN RETURN false; END IF;
  start_operation := value->>'operation' IN ('shell','code_interpreter','python_install');
  IF start_operation THEN
   IF jsonb_typeof(value->'lifecycle') IS DISTINCT FROM 'object'
   OR (value->'lifecycle') - ARRAY['poll','cancel']::text[] <> '{}'::jsonb
   OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value #> '{lifecycle,poll}'),false)
   OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value #> '{lifecycle,cancel}'),false)
   OR value #> '{lifecycle,poll}' = value #> '{lifecycle,cancel}'
   OR value #>> '{lifecycle,poll,registry_node}' IS DISTINCT FROM value->>'registry_node'
   OR value #>> '{lifecycle,cancel,registry_node}' IS DISTINCT FROM value->>'registry_node' THEN RETURN false; END IF;
  ELSIF COALESCE(value->'lifecycle','null'::jsonb) <> 'null'::jsonb THEN RETURN false; END IF;
  RETURN true;
 END IF;
 RETURN COALESCE(value->>'tier' = 'integration' AND value->>'operation' = 'invoke'
 AND value->>'provider' = 'integration.' || (value #>> '{transport,transport}') || '@1'
 AND value #>> '{transport,transport}' IN ('http','mcp','agent')
 AND COALESCE(value->'lifecycle','null'::jsonb) = 'null'::jsonb
 AND public.aidash_tool_config_is_valid(value->'transport'),false);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;

ALTER TABLE registry DROP CONSTRAINT registry_agent_config;
ALTER TABLE registry ADD CONSTRAINT registry_agent_config CHECK (kind <> 'agent' OR COALESCE(public.aidash_agent_bindings_is_valid(metadata->'config'),false)) NOT VALID;

-- Native package schemas are independent of the caller-supplied Entry schema.
-- PostgreSQL JSON Schema function DDL cannot be represented by SeaQuery.
CREATE OR REPLACE FUNCTION public.aidash_native_context_is_valid(value jsonb, kind text) RETURNS boolean
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

CREATE OR REPLACE FUNCTION public.aidash_binding_package_is_valid(value jsonb, entry_id text, entry_version text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE entity jsonb; candidate jsonb;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['entity','author','permissions','dependencies']::text[] <> '{}'::jsonb
 OR jsonb_typeof(value->'author') <> 'string' OR length(btrim(value->>'author', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) = 0
 OR jsonb_typeof(value->'permissions') <> 'array' OR jsonb_typeof(value->'dependencies') <> 'array'
 OR jsonb_path_exists(value->'permissions','strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
 entity := value->'entity';
 IF NOT COALESCE(public.aidash_package_entry_shape_is_valid(entity),false)
 OR entity ?| ARRAY['installation','binding_normalization']
 OR jsonb_typeof(entity->'id') IS DISTINCT FROM 'string' OR jsonb_typeof(entity->'version') IS DISTINCT FROM 'string'
 OR entity->>'id' IS DISTINCT FROM entry_id OR entity->>'version' IS DISTINCT FROM entry_version
 OR NOT COALESCE(public.aidash_qualified_ref_is_valid(jsonb_build_object('registry_node','aidash://contract','id',entry_id,'version',entry_version)),false) THEN RETURN false; END IF;
 FOR candidate IN SELECT jsonb_array_elements(value->'dependencies') LOOP
  IF NOT COALESCE(public.aidash_qualified_ref_is_valid(candidate || '{"registry_node":"aidash://contract"}'::jsonb),false) THEN RETURN false; END IF;
 END LOOP;
 RETURN COALESCE(CASE entity->>'kind'
 WHEN 'agent' THEN public.aidash_agent_bindings_is_valid(entity->'config')
 WHEN 'tool' THEN public.aidash_descriptor_is_valid(entity->'config') AND entity #>> '{config,tier}' <> 'builtin'
 WHEN 'bundle' THEN public.aidash_bundle_is_valid(entity->'config')
 WHEN 'memory' THEN CASE WHEN entity->'config' ? 'schema_version' THEN public.aidash_context_is_valid(entity->'config','memory') ELSE public.aidash_native_context_is_valid(entity->'config','memory') END
 WHEN 'source' THEN CASE WHEN entity->'config' ? 'schema_version' THEN public.aidash_context_is_valid(entity->'config','source') ELSE public.aidash_native_context_is_valid(entity->'config','source') END
 WHEN 'skill' THEN jsonb_typeof(entity #> '{config,instructions}') = 'string' AND length(btrim(entity #>> '{config,instructions}', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0
 ELSE false END,false);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;
