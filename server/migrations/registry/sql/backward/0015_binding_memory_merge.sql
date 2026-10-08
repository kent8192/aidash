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
  IF jsonb_typeof(item) <> 'string' OR NOT (item #>> '{}') = ANY(ARRAY['workspace_observe','workspace_wait','skill_list','skill_load','skill_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','memory_write']) THEN RETURN false; END IF;
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
  IF jsonb_typeof(config->'memory') = 'object' THEN
   INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, 'memory', 1, 'memory', config#>>'{memory,id}', config#>>'{memory,version}');
  END IF;
  INSERT INTO registry_memory_role_refs
   SELECT NEW.id, NEW.version, 'sources', ordinal::integer, 'source', value->>'id', value->>'version'
   FROM jsonb_array_elements(COALESCE(config->'sources', '[]'::jsonb)) WITH ORDINALITY AS ref(value, ordinal);
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
ALTER TABLE registry ADD CONSTRAINT registry_context_config CHECK (kind NOT IN ('memory','source') OR COALESCE(public.aidash_context_is_valid(metadata->'config',kind),false)) NOT VALID;

ALTER TABLE registry DROP CONSTRAINT registry_agent_config;
ALTER TABLE registry ADD CONSTRAINT registry_agent_config CHECK (COALESCE((((kind <> 'agent'::text) OR ((((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((length(btrim(COALESCE(((metadata -> 'config'::text) ->> 'instructions'::text), ''::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) OR
						CASE
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skills'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skills'::text)) > 0)
						ELSE false
						END) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text]))) OR ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((COALESCE(((metadata -> 'config'::text) -> 'skill_attachments'::text), '[]'::jsonb) <> '[]'::jsonb) OR (COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb) <> '[]'::jsonb)))) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'max_steps'::text)) THEN true
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_steps'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'max_steps'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((metadata -> 'config'::text) ->> 'max_steps'::text))::numeric >= (1)::numeric) AND ((((metadata -> 'config'::text) ->> 'max_steps'::text))::numeric <= (1000)::numeric))
						ELSE false
						END)) AND ((kind <> 'agent'::text) OR (((NOT ((metadata -> 'config'::text) ? 'core_capabilities'::text)) OR (
						CASE
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'core_capabilities'::text)) = 'object'::text) THEN ((((metadata -> 'config'::text) -> 'core_capabilities'::text) - ARRAY['files'::text, 'shell'::text, 'python'::text, 'patch'::text, 'skills'::text, 'sharing'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'files'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'files'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'shell'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'shell'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'python'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'python'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'patch'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'patch'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'skills'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'skills'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'sharing'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'sharing'::text)) = 'boolean'::text)))) AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'skill_attachments'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skill_attachments'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skill_attachments'::text)) <= 16)
						ELSE false
						END AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'skill_roots'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skill_roots'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skill_roots'::text)) <= 8)
						ELSE false
						END AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'reference_attachments'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'reference_attachments'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'reference_attachments'::text)) <= 8)
						ELSE false
						END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)))) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_task_creation'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_task_creation'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_task_delegation'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_task_delegation'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_memory_write'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_memory_write'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_workspace_retrieval'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_workspace_retrieval'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_cross_conversation_memory'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_cross_conversation_memory'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['model'::text, 'instructions'::text, 'tools'::text, 'skills'::text, 'cluster'::text, 'max_steps'::text, 'knowledge_digest'::text, 'core_capabilities'::text, 'skill_attachments'::text, 'skill_roots'::text, 'reference_attachments'::text, 'allow_task_creation'::text, 'allow_task_delegation'::text, 'allow_memory_write'::text, 'allow_workspace_retrieval'::text, 'allow_cross_conversation_memory'::text, 'memory'::text, 'sources'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((kind <> 'agent'::text) OR ((jsonb_typeof((metadata #> '{config,model}'::text[])) = 'object'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'version'::text)) = 'string'::text)))), false)) NOT VALID;
