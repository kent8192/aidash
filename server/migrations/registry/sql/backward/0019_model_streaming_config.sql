-- Restore the 0018_prompt_cache model constraint and installation guard.
-- Rollback precondition: no model Definition, Package or installation override
-- may carry the streaming keys. Pre-0019 binaries decode model configs with
-- deny_unknown_fields, and neither the restored registry constraint nor the
-- replaced trigger rechecks existing installation overrides. This read-only
-- guard refuses the rollback instead of leaving records no binary can read.
DO $$
DECLARE definition_count bigint; package_count bigint; installation_count bigint;
BEGIN
  SELECT count(*) INTO definition_count FROM registry
   WHERE kind = 'model' AND metadata->'config' ?| ARRAY['streaming','stream_stall_timeout_secs'];
  SELECT count(*) INTO package_count FROM packages
   WHERE manifest #>> '{entity,kind}' = 'model'
     AND manifest #> '{entity,config}' ?| ARRAY['streaming','stream_stall_timeout_secs'];
  SELECT count(*) INTO installation_count FROM installations
    JOIN registry ON registry.id = installations.id AND registry.version = installations.version
   WHERE registry.kind = 'model' AND installations.config ?| ARRAY['streaming','stream_stall_timeout_secs'];
  IF definition_count > 0 OR package_count > 0 OR installation_count > 0 THEN
    RAISE EXCEPTION 'registry 0019_model_streaming_config cannot be reversed: % Definitions, % Packages and % installation overrides use streaming or stream_stall_timeout_secs', definition_count, package_count, installation_count
      USING ERRCODE = '55000',
            HINT = 'Pre-0019 binaries cannot read these records; remove the streaming settings or restore the pre-upgrade database backup instead of reversing this migration.';
  END IF;
END $$;
ALTER TABLE registry DROP CONSTRAINT registry_model_streaming;
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''request_timeout_secs''::text, ''streaming''::text, ''stream_stall_timeout_secs''::text, ''media_routes''::text', '''request_timeout_secs''::text, ''media_routes''::text');
  IF edited = definition THEN RAISE EXCEPTION 'Streaming rollback allowlist anchor missing: registry_model_config'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;
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
