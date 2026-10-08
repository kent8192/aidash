CREATE OR REPLACE FUNCTION public.aidash_qualified_ref_is_valid(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT COALESCE(jsonb_typeof(value) = 'object'
 AND value - ARRAY['registry_node','id','version']::text[] = '{}'::jsonb
 AND jsonb_typeof(value->'registry_node') = 'string' AND jsonb_typeof(value->'id') = 'string' AND jsonb_typeof(value->'version') = 'string'
 AND value->>'registry_node' ~ '^aidash://[A-Za-z0-9-]{1,100}$'
 AND value->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'
 AND value->>'version' ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?([+][0-9A-Za-z.-]+)?$', false)
$$;

-- PostgreSQL function bodies and NOT VALID checks lack typed operations.
-- DDL only: retain historical rows without converting Agent configurations.
CREATE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
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
CREATE FUNCTION public.aidash_context_is_valid(value jsonb, kind text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE source jsonb; adapter text;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['schema_version','source']::text[] <> '{}'::jsonb
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb OR jsonb_typeof(value->'source') <> 'object' THEN RETURN false; END IF;
 source := value->'source'; adapter := source->>'adapter';
 IF kind = 'memory' THEN RETURN adapter IN ('conversation_memory','semantic_memory') AND source - 'adapter' = '{}'::jsonb; END IF;
 IF kind <> 'source' THEN RETURN false; END IF;
 RETURN CASE adapter
 WHEN 'workspace_retrieval' THEN source - 'adapter' = '{}'::jsonb
 WHEN 'private_references' THEN source - ARRAY['adapter','digest']::text[] = '{}'::jsonb AND source->>'digest' ~ '^[a-fA-F0-9]{64}$'
 WHEN 'reference_attachments' THEN source - ARRAY['adapter','references']::text[] = '{}'::jsonb AND jsonb_typeof(source->'references') = 'array' AND jsonb_array_length(source->'references') BETWEEN 1 AND 8
 WHEN 'skill_attachments' THEN source - ARRAY['adapter','attachments']::text[] = '{}'::jsonb AND jsonb_typeof(source->'attachments') = 'array' AND jsonb_array_length(source->'attachments') BETWEEN 1 AND 16
 WHEN 'skill_roots' THEN source - ARRAY['adapter','roots']::text[] = '{}'::jsonb AND jsonb_typeof(source->'roots') = 'array' AND jsonb_array_length(source->'roots') BETWEEN 1 AND 8 AND NOT jsonb_path_exists(source->'roots','strict $[*] ? (@.type() != "string")')
 ELSE false END;
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;
ALTER TABLE registry DROP CONSTRAINT registry_agent_config;
ALTER TABLE registry ADD CONSTRAINT registry_agent_config CHECK (kind <> 'agent' OR COALESCE(public.aidash_agent_bindings_is_valid(metadata->'config'),false)) NOT VALID;
ALTER TABLE registry DROP CONSTRAINT registry_metadata_shape;
ALTER TABLE registry ADD CONSTRAINT registry_metadata_shape CHECK (((jsonb_typeof((metadata - ARRAY['installation','binding_normalization']::text[])) = 'object'::text) AND (jsonb_typeof(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'name'::text)) = 'object'::text) AND (((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - ARRAY['installation','binding_normalization']::text[]), 'strict $."name".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'description'::text)) = 'object'::text) AND (((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - ARRAY['installation','binding_normalization']::text[]), 'strict $."description".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'config'::text)) = 'object'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'schema'::text), '{}'::jsonb)) = 'object'::text) AND public.jsonschema_is_valid((COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'schema'::text), '{}'::jsonb))::json) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND
						CASE
						WHEN (jsonb_typeof((metadata - ARRAY['installation','binding_normalization']::text[])) = 'object'::text) THEN (((metadata - ARRAY['installation','binding_normalization']::text[]) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation','binding_normalization']::text[]) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((NOT (metadata ? 'installation'::text)) OR ((jsonb_typeof((metadata -> 'installation'::text)) = 'object'::text) AND (((metadata -> 'installation'::text) ->> 'contract'::text) = '1'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'tenant'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'installation'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'revision'::text)) = 'number'::text))))) NOT VALID;
ALTER TABLE registry DROP CONSTRAINT registry_tool_config;
ALTER TABLE registry ADD CONSTRAINT registry_tool_config CHECK (kind <> 'tool' OR COALESCE(public.aidash_descriptor_is_valid(metadata->'config'),false)) NOT VALID;
ALTER TABLE registry ADD CONSTRAINT registry_context_config CHECK (kind NOT IN ('memory','source') OR COALESCE(public.aidash_context_is_valid(metadata->'config',kind),false)) NOT VALID;
CREATE FUNCTION public.aidash_package_entry_shape_is_valid(metadata jsonb) RETURNS boolean LANGUAGE sql IMMUTABLE STRICT AS $$ SELECT COALESCE(((jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'name'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."name".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'description'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."description".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'config'::text)) = 'object'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb)) = 'object'::text) AND public.jsonschema_is_valid((COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb))::json) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND
						CASE
						WHEN (jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) THEN (((metadata - 'installation'::text) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((NOT (metadata ? 'installation'::text)) OR ((jsonb_typeof((metadata -> 'installation'::text)) = 'object'::text) AND (((metadata -> 'installation'::text) ->> 'contract'::text) = '1'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'tenant'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'installation'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'revision'::text)) = 'number'::text)))),false) $$;
CREATE FUNCTION public.aidash_binding_package_is_valid(value jsonb, entry_id text, entry_version text) RETURNS boolean
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
 WHEN 'memory' THEN public.aidash_context_is_valid(entity->'config','memory')
 WHEN 'source' THEN public.aidash_context_is_valid(entity->'config','source')
 WHEN 'skill' THEN jsonb_typeof(entity #> '{config,instructions}') = 'string' AND length(btrim(entity #>> '{config,instructions}', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0
 ELSE false END,false);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;
ALTER TABLE packages DROP CONSTRAINT packages_identity;
ALTER TABLE packages ADD CONSTRAINT packages_identity CHECK (public.aidash_binding_package_is_valid(manifest,id,version)) NOT VALID;

CREATE OR REPLACE FUNCTION public.guard_installation_config() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
DECLARE
    target_kind text;
    target_config jsonb;
BEGIN
    SELECT kind, metadata->'config' INTO target_kind, target_config
    FROM registry
    WHERE id = NEW.id AND version = NEW.version;

    IF target_kind = 'model' AND NOT COALESCE(
        (CASE WHEN (NEW.config->'request_timeout_secs') IS NULL OR (NEW.config->'request_timeout_secs') = 'null'::jsonb THEN true WHEN jsonb_typeof(NEW.config->'request_timeout_secs') = 'number' AND (NEW.config->'request_timeout_secs')::text ~ '^[1-9][0-9]*$' THEN (NEW.config->'request_timeout_secs')::text::numeric BETWEEN 1 AND 4294967295 ELSE false END) AND
        (NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','media_routes']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'provider' OR (jsonb_typeof(NEW.config->'provider') = 'string' AND NEW.config->>'provider' = 'openrouter'))
        AND (NOT NEW.config ? 'model_id' OR (jsonb_typeof(NEW.config->'model_id') = 'string' AND length(btrim(NEW.config->>'model_id', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND (NOT NEW.config ? 'endpoint' OR (jsonb_typeof(NEW.config->'endpoint') = 'string' AND length(btrim(NEW.config->>'endpoint', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND (NOT NEW.config ? 'credential_env' OR jsonb_typeof(NEW.config->'credential_env') IN ('string','null'))
        AND (NOT NEW.config ? 'reasoning_effort' OR NEW.config->'reasoning_effort' = 'null'::jsonb OR NEW.config->>'reasoning_effort' IN ('none','minimal','low','medium','high','xhigh','max'))
        AND (NOT NEW.config ? 'context_window' OR (jsonb_typeof(NEW.config->'context_window') = 'number' AND NEW.config->>'context_window' ~ '^(0|[1-9][0-9]*)$' AND (NEW.config->>'context_window')::numeric >= 2048 AND (NEW.config->>'context_window')::numeric <= 18446744073709551615))
        AND (NOT NEW.config ? 'media_routes' OR aidash_media_routes_valid(NEW.config->'media_routes'))
        AND (NOT NEW.config ? 'modalities' OR (jsonb_typeof(NEW.config->'modalities') = 'array' AND NEW.config->'modalities' @> '["text"]'::jsonb AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.config->'modalities') AS item WHERE jsonb_typeof(item) <> 'string')))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid model configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'agent' AND NOT COALESCE(
        NEW.config - ARRAY['model','instructions','bindings','remove_default','cluster','max_steps']::text[] = '{}'::jsonb
        AND public.aidash_agent_bindings_is_valid(target_config || NEW.config), false) THEN
        RAISE EXCEPTION 'installation override is not a valid Binding configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'agent' AND NEW.config ? 'model' AND NOT EXISTS (
        SELECT 1 FROM registry model_record
        WHERE model_record.id = NEW.config#>>'{model,id}'
          AND model_record.version = NEW.config#>>'{model,version}'
          AND model_record.kind = 'model'
    ) THEN
        RAISE EXCEPTION 'installed agent model override must reference a registered model'
            USING ERRCODE = '23514', CONSTRAINT = 'registry_agent_model_installation_reference';
    END IF;

    IF target_kind = 'cluster' AND NOT COALESCE(
        (NEW.config - ARRAY['coordinator']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'coordinator' OR (jsonb_typeof(NEW.config->'coordinator') = 'object' AND jsonb_typeof(NEW.config->'coordinator'->'id') = 'string' AND jsonb_typeof(NEW.config->'coordinator'->'version') = 'string'))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid cluster configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'skill' AND NOT COALESCE(
        (NEW.config - ARRAY['instructions']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'instructions' OR (jsonb_typeof(NEW.config->'instructions') = 'string' AND length(btrim(NEW.config->>'instructions', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid skill configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'tool' AND NOT COALESCE(
        NEW.config - ARRAY['transport','narrow']::text[] = '{}'::jsonb AND public.aidash_descriptor_is_valid(target_config || NEW.config), false
    ) THEN
        RAISE EXCEPTION 'installation override is not a valid tool configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    RETURN NEW;
END $_$;
