-- PostgreSQL function bodies and NOT VALID constraints have no typed migration
-- operation. These statements are DDL only; historical definitions are retained.
CREATE FUNCTION public.aidash_qualified_ref_is_valid(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT COALESCE(jsonb_typeof(value) = 'object'
 AND value - ARRAY['registry_node','id','version']::text[] = '{}'::jsonb
 AND value->>'registry_node' ~ '^aidash://[A-Za-z0-9-]{1,100}$'
 AND value->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'
 AND value->>'version' ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?([+][0-9A-Za-z.-]+)?$', false)
$$;
CREATE FUNCTION public.aidash_bundle_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE candidate jsonb;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - 'members' <> '{}'::jsonb
 OR jsonb_typeof(value->'members') <> 'array' THEN RETURN false; END IF;
 IF jsonb_array_length(value->'members') NOT BETWEEN 1 AND 128 THEN RETURN false; END IF;
 FOR candidate IN SELECT jsonb_array_elements(value->'members') LOOP
  IF NOT public.aidash_qualified_ref_is_valid(candidate) THEN RETURN false; END IF;
 END LOOP;
 RETURN (SELECT count(*) = count(DISTINCT elements.item)
 FROM jsonb_array_elements(value->'members') AS elements(item));
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;
CREATE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
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
  WHEN 'artifact_publish' THEN 'core.artifacts@1' WHEN 'memory_write' THEN 'core.memory@1'
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
   OR value->'lifecycle' - ARRAY['poll','cancel']::text[] <> '{}'::jsonb
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
ALTER TABLE registry DROP CONSTRAINT registry_tool_config;
ALTER TABLE registry ADD CONSTRAINT registry_tool_config CHECK (kind <> 'tool' OR COALESCE(public.aidash_descriptor_is_valid(metadata->'config') OR public.aidash_tool_config_is_valid(metadata->'config'),false)) NOT VALID;
ALTER TABLE registry ADD CONSTRAINT registry_bundle_config CHECK (kind <> 'bundle' OR COALESCE(public.aidash_bundle_is_valid(metadata->'config'),false)) NOT VALID;
ALTER TABLE packages DROP CONSTRAINT packages_identity;
ALTER TABLE packages ADD CONSTRAINT packages_identity CHECK (COALESCE(((jsonb_typeof(manifest) = 'object'::text) AND ((manifest #> '{entity,id}'::text[]) = to_jsonb(id)) AND ((manifest #> '{entity,version}'::text[]) = to_jsonb(version)) AND
						CASE
						WHEN (jsonb_typeof(manifest) = 'object'::text) THEN ((manifest - ARRAY['entity'::text, 'author'::text, 'permissions'::text, 'dependencies'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof((manifest -> 'author'::text)) = 'string'::text) AND (length(btrim((manifest ->> 'author'::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0)) AND ((jsonb_typeof((manifest -> 'permissions'::text)) = 'array'::text) AND (NOT jsonb_path_exists((manifest -> 'permissions'::text), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof((manifest -> 'dependencies'::text)) = 'array'::text) AND (NOT jsonb_path_exists((manifest -> 'dependencies'::text), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true))) AND
						CASE
						WHEN (jsonb_typeof((manifest -> 'entity'::text)) = 'object'::text) THEN (((manifest -> 'entity'::text) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'id'::text)) = 'string'::text) AND (((manifest -> 'entity'::text) ->> 'id'::text) ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text)) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'version'::text)) = 'string'::text) AND
						CASE
						WHEN (((manifest -> 'entity'::text) ->> 'version'::text) ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(((manifest -> 'entity'::text) ->> 'version'::text), '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(((manifest -> 'entity'::text) ->> 'version'::text), '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(((manifest -> 'entity'::text) ->> 'version'::text), '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
						ELSE false
						END) AND (((manifest -> 'entity'::text) ->> 'kind'::text) = ANY (ARRAY['agent'::text, 'tool'::text, 'skill'::text, 'bundle'::text])) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'name'::text)) = 'object'::text) AND (((manifest -> 'entity'::text) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists(((manifest -> 'entity'::text) -> 'name'::text), 'strict $.*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'description'::text)) = 'object'::text) AND (((manifest -> 'entity'::text) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists(((manifest -> 'entity'::text) -> 'description'::text), 'strict $.*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((manifest -> 'entity'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((manifest -> 'entity'::text) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(((manifest -> 'entity'::text) -> 'schema'::text)) = 'object'::text) AND public.jsonschema_is_valid((((manifest -> 'entity'::text) -> 'schema'::text))::json) AND (jsonb_typeof(((manifest -> 'entity'::text) -> 'config'::text)) = 'object'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (((NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'core_capabilities'::text)) OR (
						CASE
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text)) = 'object'::text) THEN (((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) - ARRAY['files'::text, 'shell'::text, 'python'::text, 'patch'::text, 'skills'::text, 'sharing'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'files'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'files'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'shell'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'shell'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'python'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'python'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'patch'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'patch'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'skills'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'skills'::text)) = 'boolean'::text)) AND ((NOT ((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) ? 'sharing'::text)) OR (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'core_capabilities'::text) -> 'sharing'::text)) = 'boolean'::text)))) AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'skill_attachments'::text)) THEN true
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_attachments'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_attachments'::text)) <= 16)
						ELSE false
						END AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'skill_roots'::text)) THEN true
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text)) <= 8)
						ELSE false
						END AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'reference_attachments'::text)) THEN true
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'reference_attachments'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'reference_attachments'::text)) <= 8)
						ELSE false
						END AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (
						CASE
						WHEN (jsonb_typeof(((manifest -> 'entity'::text) -> 'config'::text)) = 'object'::text) THEN ((((manifest -> 'entity'::text) -> 'config'::text) - ARRAY['model'::text, 'instructions'::text, 'tools'::text, 'skills'::text, 'cluster'::text, 'max_steps'::text, 'knowledge_digest'::text, 'core_capabilities'::text, 'skill_attachments'::text, 'skill_roots'::text, 'reference_attachments'::text, 'allow_task_creation'::text, 'allow_task_delegation'::text, 'allow_memory_write'::text, 'allow_workspace_retrieval'::text, 'allow_cross_conversation_memory'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text)) = 'object'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'model'::text) -> 'version'::text)) = 'string'::text) AND (((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((length(btrim(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) ->> 'instructions'::text), ''::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) OR
						CASE
						WHEN (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text)) = 'array'::text) THEN (jsonb_array_length((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text)) > 0)
						ELSE false
						END) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text]))) OR ((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_attachments'::text), '[]'::jsonb) <> '[]'::jsonb) OR (COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb) <> '[]'::jsonb)))) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND
						CASE
						WHEN (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'max_steps'::text)) THEN true
						WHEN ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'max_steps'::text)) = 'number'::text) AND ((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text))::numeric >= (1)::numeric) AND (((((manifest -> 'entity'::text) -> 'config'::text) ->> 'max_steps'::text))::numeric <= (1000)::numeric))
						ELSE false
						END AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((((manifest -> 'entity'::text) -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_task_creation'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_task_creation'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_task_delegation'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_task_delegation'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_memory_write'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_memory_write'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_workspace_retrieval'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_workspace_retrieval'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'agent'::text) OR (NOT (((manifest -> 'entity'::text) -> 'config'::text) ? 'allow_cross_conversation_memory'::text)) OR (jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'allow_cross_conversation_memory'::text)) = 'boolean'::text)) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'skill'::text) OR ((jsonb_typeof((((manifest -> 'entity'::text) -> 'config'::text) -> 'instructions'::text)) = 'string'::text) AND (length(btrim((((manifest -> 'entity'::text) -> 'config'::text) ->> 'instructions'::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0))) AND ((((manifest -> 'entity'::text) ->> 'kind'::text) <> 'tool'::text) OR COALESCE(public.aidash_descriptor_is_valid(((manifest -> 'entity'::text) -> 'config'::text)) OR public.aidash_tool_config_is_valid(((manifest -> 'entity'::text) -> 'config'::text)), false))), false) AND ((manifest #>> '{entity,kind}') <> 'bundle' OR public.aidash_bundle_is_valid(manifest #> '{entity,config}'))) NOT VALID;
