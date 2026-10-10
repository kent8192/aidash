ALTER TABLE registry DROP CONSTRAINT registry_model_config;
ALTER TABLE registry ADD CONSTRAINT registry_model_config CHECK (COALESCE((((kind <> 'model'::text) OR (((metadata #>> '{config,provider}'::text[]) = 'openrouter'::text) AND (jsonb_typeof((metadata #> '{config,model_id}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,model_id}'::text[]), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND public.aidash_valid_http_endpoint((metadata #> '{config,endpoint}'::text[])) AND
						CASE
						WHEN (jsonb_typeof((metadata #> '{config,context_window}'::text[])) = 'number'::text) THEN ((((metadata #>> '{config,context_window}'::text[]))::numeric >= (2048)::numeric) AND (trunc(((metadata #>> '{config,context_window}'::text[]))::numeric) = ((metadata #>> '{config,context_window}'::text[]))::numeric))
						ELSE false
						END AND (jsonb_typeof((metadata #> '{config,modalities}'::text[])) = 'array'::text) AND ((metadata #> '{config,modalities}'::text[]) @> '["text"]'::jsonb) AND ((NOT ((metadata -> 'config'::text) ? 'reasoning_effort'::text)) OR ((metadata #> '{config,reasoning_effort}'::text[]) = 'null'::jsonb) OR ((metadata #>> '{config,reasoning_effort}'::text[]) = ANY (ARRAY['none'::text, 'minimal'::text, 'low'::text, 'medium'::text, 'high'::text, 'xhigh'::text, 'max'::text]))))) AND ((kind <> 'model'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'model_id'::text, 'endpoint'::text, 'credential_env'::text, 'provider_credential'::text, 'reasoning_effort'::text, 'context_window'::text, 'max_output_tokens'::text, 'modalities'::text, 'cost'::text, 'request_timeout_secs'::text, 'streaming'::text, 'stream_stall_timeout_secs'::text, 'media_routes'::text, 'projection_versions'::text]) = '{}'::jsonb)
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
						END) AND ((kind <> 'model'::text) OR
						CASE
						WHEN (((metadata #> '{config,streaming}'::text[]) IS NULL) OR ((metadata #> '{config,streaming}'::text[]) = 'null'::jsonb)) THEN true
						ELSE (jsonb_typeof((metadata #> '{config,streaming}'::text[])) = 'boolean'::text)
						END) AND ((kind <> 'model'::text) OR
						CASE
						WHEN (((metadata #> '{config,stream_stall_timeout_secs}'::text[]) IS NULL) OR ((metadata #> '{config,stream_stall_timeout_secs}'::text[]) = 'null'::jsonb)) THEN true
						WHEN ((jsonb_typeof((metadata #> '{config,stream_stall_timeout_secs}'::text[])) = 'number'::text) AND (((metadata #> '{config,stream_stall_timeout_secs}'::text[]))::text ~ '^[1-9][0-9]*$'::text)) THEN (((((metadata #> '{config,stream_stall_timeout_secs}'::text[]))::text)::numeric >= (1)::numeric) AND ((((metadata #> '{config,stream_stall_timeout_secs}'::text[]))::text)::numeric <= ('4294967295'::bigint)::numeric))
						ELSE false
						END) AND ((kind <> 'model'::text) OR ((NOT ((metadata -> 'config'::text) ? 'media_routes'::text)) OR public.aidash_media_routes_valid((metadata #> '{config,media_routes}'::text[])))) AND ((kind <> 'model'::text) OR (NOT ((metadata -> 'config'::text) ? 'projection_versions'::text)) OR ((jsonb_typeof((metadata #> '{config,projection_versions}'::text[])) = 'array'::text) AND ('["legacy", "ordered", "native"]'::jsonb @> (metadata #> '{config,projection_versions}'::text[]))))), false));
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
        (CASE WHEN (NEW.config->'stream_stall_timeout_secs') IS NULL OR (NEW.config->'stream_stall_timeout_secs') = 'null'::jsonb THEN true WHEN jsonb_typeof(NEW.config->'stream_stall_timeout_secs') = 'number' AND (NEW.config->'stream_stall_timeout_secs')::text ~ '^[1-9][0-9]*$' THEN (NEW.config->'stream_stall_timeout_secs')::text::numeric BETWEEN 1 AND 4294967295 ELSE false END) AND
        (NOT NEW.config ? 'streaming' OR jsonb_typeof(NEW.config->'streaming') IN ('boolean','null')) AND
        (NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','streaming','stream_stall_timeout_secs','media_routes']::text[]) = '{}'::jsonb
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
