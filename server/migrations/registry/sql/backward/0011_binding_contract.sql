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
        (NEW.config - ARRAY['model','instructions','tools','skills','cluster','max_steps']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'model' OR (jsonb_typeof(NEW.config->'model') = 'object' AND jsonb_typeof(NEW.config->'model'->'id') = 'string' AND jsonb_typeof(NEW.config->'model'->'version') = 'string'))
        AND (NOT NEW.config ? 'instructions' OR (jsonb_typeof(NEW.config->'instructions') = 'string' AND length(btrim(NEW.config->>'instructions', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND (NOT NEW.config ? 'tools' OR (jsonb_typeof(NEW.config->'tools') = 'array' AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.config->'tools') AS item WHERE jsonb_typeof(item) <> 'object' OR jsonb_typeof(item->'id') <> 'string' OR jsonb_typeof(item->'version') <> 'string')))
        AND (NOT NEW.config ? 'skills' OR (jsonb_typeof(NEW.config->'skills') = 'array' AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.config->'skills') AS item WHERE jsonb_typeof(item) <> 'object' OR jsonb_typeof(item->'id') <> 'string' OR jsonb_typeof(item->'version') <> 'string')))
        AND (NOT NEW.config ? 'cluster' OR NEW.config->'cluster' = 'null'::jsonb OR (jsonb_typeof(NEW.config->'cluster') = 'object' AND jsonb_typeof(NEW.config->'cluster'->'id') = 'string' AND jsonb_typeof(NEW.config->'cluster'->'version') = 'string'))
        AND (NOT NEW.config ? 'max_steps' OR (jsonb_typeof(NEW.config->'max_steps') = 'number' AND NEW.config->>'max_steps' ~ '^(0|[1-9][0-9]*)$' AND (NEW.config->>'max_steps')::numeric BETWEEN 1 AND 1000))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid agent configuration'
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
        aidash_tool_config_is_valid(target_config || NEW.config), false
    ) THEN
        RAISE EXCEPTION 'installation override is not a valid tool configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    RETURN NEW;
END $_$;

-- DDL reversal only; no historical configuration conversion.
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

ALTER TABLE registry DROP CONSTRAINT registry_context_config;
ALTER TABLE registry DROP CONSTRAINT registry_tool_config;
ALTER TABLE registry ADD CONSTRAINT registry_tool_config CHECK (kind <> 'tool' OR COALESCE(public.aidash_descriptor_is_valid(metadata->'config') OR public.aidash_tool_config_is_valid(metadata->'config'),false)) NOT VALID;
ALTER TABLE registry DROP CONSTRAINT registry_metadata_shape;
ALTER TABLE registry ADD CONSTRAINT registry_metadata_shape CHECK (((jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'name'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."name".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'description'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."description".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'config'::text)) = 'object'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb)) = 'object'::text) AND public.jsonschema_is_valid((COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb))::json) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND
						CASE
						WHEN (jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) THEN (((metadata - 'installation'::text) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((NOT (metadata ? 'installation'::text)) OR ((jsonb_typeof((metadata -> 'installation'::text)) = 'object'::text) AND (((metadata -> 'installation'::text) ->> 'contract'::text) = '1'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'tenant'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'installation'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'revision'::text)) = 'number'::text))))) NOT VALID;
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
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['model'::text, 'instructions'::text, 'tools'::text, 'skills'::text, 'cluster'::text, 'max_steps'::text, 'knowledge_digest'::text, 'core_capabilities'::text, 'skill_attachments'::text, 'skill_roots'::text, 'reference_attachments'::text, 'allow_task_creation'::text, 'allow_task_delegation'::text, 'allow_memory_write'::text, 'allow_workspace_retrieval'::text, 'allow_cross_conversation_memory'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((kind <> 'agent'::text) OR ((jsonb_typeof((metadata #> '{config,model}'::text[])) = 'object'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'version'::text)) = 'string'::text)))), false)) NOT VALID;
DROP FUNCTION public.aidash_binding_package_is_valid(jsonb,text,text);
DROP FUNCTION public.aidash_package_entry_shape_is_valid(jsonb);
DROP FUNCTION public.aidash_context_is_valid(jsonb,text);
DROP FUNCTION public.aidash_agent_bindings_is_valid(jsonb);

CREATE OR REPLACE FUNCTION public.aidash_qualified_ref_is_valid(value jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT COALESCE(jsonb_typeof(value) = 'object'
 AND value - ARRAY['registry_node','id','version']::text[] = '{}'::jsonb
 AND value->>'registry_node' ~ '^aidash://[A-Za-z0-9-]{1,100}$'
 AND value->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'
 AND value->>'version' ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?([+][0-9A-Za-z.-]+)?$', false)
$$;
