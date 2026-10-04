SET LOCAL search_path = public, pg_catalog;

SET LOCAL check_function_bodies = false;
CREATE FUNCTION public.aidash_media_routes_valid(routes jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE
    route jsonb;
    media_format jsonb;
    verified_at timestamptz;
    expires_at timestamptz;
BEGIN
    IF jsonb_typeof(routes) <> 'array' THEN RETURN false; END IF;
    FOR route IN SELECT value FROM jsonb_array_elements(routes) AS entries(value) LOOP
        IF jsonb_typeof(route) <> 'object' THEN RETURN false; END IF;
        IF NOT route ?& ARRAY['tag','formats','source','verified_at','expires_at']::text[]
           OR (route - ARRAY['tag','formats','source','verified_at','expires_at']::text[]) <> '{}'::jsonb
           OR jsonb_typeof(route->'tag') <> 'string'
           OR jsonb_typeof(route->'formats') <> 'array'
           OR jsonb_typeof(route->'source') <> 'string'
           OR jsonb_typeof(route->'verified_at') <> 'string'
           OR jsonb_typeof(route->'expires_at') <> 'string'
        THEN RETURN false; END IF;
        IF route->>'tag' !~ '^[A-Za-z0-9._/-]+$'
           OR length(btrim(route->>'source')) = 0
           OR jsonb_array_length(route->'formats') = 0
        THEN RETURN false; END IF;
        FOR media_format IN SELECT value FROM jsonb_array_elements(route->'formats') AS formats(value) LOOP
            IF jsonb_typeof(media_format) <> 'string'
               OR media_format #>> '{}' NOT IN ('image/png','image/jpeg','image/gif','image/webp','wav','mp3','m4a','aac','ogg','webm','flac')
            THEN RETURN false; END IF;
        END LOOP;
        IF route->>'verified_at' !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]{1,9})?(Z|[+-][0-9]{2}:[0-9]{2})$'
           OR route->>'expires_at' !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]{1,9})?(Z|[+-][0-9]{2}:[0-9]{2})$'
        THEN RETURN false; END IF;
        BEGIN
            verified_at := (route->>'verified_at')::timestamptz;
            expires_at := (route->>'expires_at')::timestamptz;
        EXCEPTION WHEN OTHERS THEN
            RETURN false;
        END;
        IF NOT isfinite(verified_at) OR NOT isfinite(expires_at) OR verified_at >= expires_at
        THEN RETURN false; END IF;
    END LOOP;
    RETURN true;
END $_$;
CREATE FUNCTION public.aidash_package_source_matches(input_manifest jsonb, input_source text) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE
    AS $$
BEGIN
    IF input_source IS NULL THEN
        RETURN false;
    END IF;
    RETURN input_source::jsonb = input_manifest;
EXCEPTION WHEN OTHERS THEN
    RETURN false;
END
$$;
CREATE FUNCTION public.aidash_tool_config_is_valid(input_value jsonb) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $_$ SELECT (CASE input_value->>'transport' WHEN 'native' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','operation','allowed_hosts']::text[]) = '{}'::jsonb ELSE false END AND jsonb_typeof(input_value->'operation') = 'string' AND input_value->>'operation' IN ('echo', 'http_get') AND (NOT (input_value ? 'allowed_hosts') OR jsonb_typeof(input_value->'allowed_hosts') = 'array' AND NOT jsonb_path_exists(input_value->'allowed_hosts', 'strict $[*] ? (@.type() != "string")', '{}'::jsonb, true)) AND (input_value->>'operation' <> 'http_get' OR CASE WHEN jsonb_typeof(COALESCE(input_value->'allowed_hosts', '[]'::jsonb)) = 'array' THEN jsonb_array_length(COALESCE(input_value->'allowed_hosts', '[]'::jsonb)) > 0 ELSE false END)) WHEN 'http' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','endpoint','credential_env','replay']::text[]) = '{}'::jsonb ELSE false END AND aidash_valid_http_endpoint(input_value->'endpoint') AND jsonb_typeof(COALESCE(input_value->'credential_env', 'null'::jsonb)) IN ('string', 'null') AND (jsonb_typeof(COALESCE(input_value->'credential_env', 'null'::jsonb)) <> 'string' OR input_value->>'credential_env' ~ '^AIDASH_SECRET_[A-Z0-9_]*$') AND jsonb_typeof(input_value->'replay') = 'string' AND input_value->>'replay' IN ('read_only', 'idempotent', 'unsafe')) WHEN 'mcp' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','endpoint','credential_env','tool_name','replay','idempotency_argument']::text[]) = '{}'::jsonb ELSE false END AND aidash_valid_http_endpoint(input_value->'endpoint') AND jsonb_typeof(COALESCE(input_value->'credential_env', 'null'::jsonb)) IN ('string', 'null') AND (jsonb_typeof(COALESCE(input_value->'credential_env', 'null'::jsonb)) <> 'string' OR input_value->>'credential_env' ~ '^AIDASH_SECRET_[A-Z0-9_]*$') AND jsonb_typeof(input_value->'tool_name') = 'string' AND jsonb_typeof(input_value->'replay') = 'string' AND input_value->>'replay' IN ('read_only', 'idempotent', 'unsafe') AND jsonb_typeof(COALESCE(input_value->'idempotency_argument', 'null'::jsonb)) IN ('string', 'null') AND (input_value->>'replay' <> 'idempotent' OR (jsonb_typeof(input_value->'idempotency_argument') = 'string' AND length(btrim(input_value->>'idempotency_argument', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))) WHEN 'agent' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','node_id','agent']::text[]) = '{}'::jsonb ELSE false END AND jsonb_typeof(input_value->'node_id') = 'string' AND input_value->>'node_id' ~ '^aidash://[A-Za-z0-9-]{1,100}$' AND jsonb_typeof(input_value->'agent') = 'object' AND jsonb_typeof(input_value->'agent'->'id') = 'string' AND jsonb_typeof(input_value->'agent'->'version') = 'string' AND input_value->'agent'->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$' AND CASE WHEN input_value->'agent'->>'version' ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$' THEN split_part(input_value->'agent'->>'version', '.', 1)::numeric <= 18446744073709551615 AND split_part(input_value->'agent'->>'version', '.', 2)::numeric <= 18446744073709551615 AND split_part(split_part(split_part(input_value->'agent'->>'version', '.', 3), '-', 1), '+', 1)::numeric <= 18446744073709551615 ELSE false END) ELSE false END) $_$;
CREATE FUNCTION public.aidash_valid_http_endpoint(input_value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE
    AS $_$
DECLARE
    endpoint text;
    authority text;
    host text;
    port_text text;
BEGIN
    IF input_value IS NULL OR jsonb_typeof(input_value) <> 'string' THEN
        RETURN false;
    END IF;
    endpoint := input_value #>> '{}';
    IF endpoint !~* '^https?://' OR strpos(endpoint, '?') > 0 OR strpos(endpoint, '#') > 0
       OR endpoint ~ '[[:cntrl:]]'
       OR endpoint ~ '%([^0-9A-Fa-f]|$)'
       OR endpoint ~ '%[0-9A-Fa-f]([^0-9A-Fa-f]|$)' THEN
        RETURN false;
    END IF;

    endpoint := regexp_replace(endpoint, '^https?://', '', 'i');
    authority := split_part(endpoint, '/', 1);
    IF authority = '' OR authority ~ '[@[:space:]]' THEN
        RETURN false;
    END IF;

    IF left(authority, 1) = '[' THEN
        IF authority !~ '^\[[0-9A-Fa-f:.]+\](:[0-9]*)?$' THEN
            RETURN false;
        END IF;
        host := substring(authority FROM 2 FOR strpos(authority, ']') - 2);
        IF strpos(host, ':') = 0 THEN
            RETURN false;
        END IF;
        PERFORM host::inet;
        IF authority ~ ':[0-9]+$' THEN
            port_text := regexp_replace(authority, '^.*:', '');
        END IF;
    ELSE
        IF authority ~ ':[0-9]*$' THEN
            port_text := regexp_replace(authority, '^.*:', '');
            IF port_text = '' THEN
                port_text := NULL;
            END IF;
            host := regexp_replace(authority, ':[0-9]*$', '');
        ELSE
            host := authority;
        END IF;
        IF host !~ '^([[:alnum:]_]([[:alnum:]_-]*[[:alnum:]_])?)(\.([[:alnum:]_]([[:alnum:]_-]*[[:alnum:]_])?))*\.?$' THEN
            RETURN false;
        END IF;
        IF host ~ '^[0-9.]+$' THEN
            -- Do not let an invalid numeric IPv4 literal pass as a DNS name.
            PERFORM host::inet;
        END IF;
    END IF;

    IF port_text IS NOT NULL AND port_text::numeric NOT BETWEEN 0 AND 65535 THEN
        RETURN false;
    END IF;
    RETURN true;
EXCEPTION WHEN OTHERS THEN
    RETURN false;
END
$_$;
CREATE FUNCTION public.guard_installation_config() RETURNS trigger
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
CREATE FUNCTION public.guard_registry_agent_model_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        RAISE EXCEPTION 'registry_agent_model_refs is maintained by registry'
            USING ERRCODE = '23514', CONSTRAINT = 'registry_agent_model_reference';
    END IF;
    RETURN NULL;
END $$;
CREATE FUNCTION public.guard_registry_agent_resource_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        RAISE EXCEPTION 'registry_agent_resource_refs is maintained by registry'
            USING ERRCODE = '23514', CONSTRAINT = 'registry_agent_resource_reference';
    END IF;
    RETURN NULL;
END $$;
CREATE FUNCTION public.lock_registry_installation_writes() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Acquire one schema-scoped lock before either table takes row locks. This
    -- gives base-record edits and installation edits a consistent lock order.
    PERFORM pg_advisory_xact_lock(70721023, hashtext(TG_TABLE_SCHEMA));
    RETURN NULL;
END $$;
CREATE FUNCTION public.marketplace_registry_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.metadata ? 'installation' OR (TG_OP='UPDATE' AND OLD.metadata ? 'installation') OR EXISTS (SELECT 1 FROM registry WHERE id=NEW.id AND version=NEW.version AND metadata ? 'installation') THEN
  IF TG_OP <> 'INSERT' OR current_setting('aidash.marketplace_writer', true) IS DISTINCT FROM '1'
    OR NEW.metadata->'installation'->>'contract' IS DISTINCT FROM '1'
    OR NEW.id NOT LIKE 'mkt-%' THEN
   RAISE EXCEPTION 'unsupported Marketplace writer';
  END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE FUNCTION public.sync_registry_agent_model_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.kind = 'agent' THEN
        DELETE FROM registry_agent_model_refs
        WHERE agent_id = OLD.id AND agent_version = OLD.version;
    END IF;
    IF NEW.kind = 'agent' THEN
        INSERT INTO registry_agent_model_refs(agent_id, agent_version, model_id, model_version)
        VALUES (NEW.id, NEW.version, NEW.metadata#>>'{config,model,id}', NEW.metadata#>>'{config,model,version}');
    END IF;
    RETURN NEW;
END $$;
CREATE FUNCTION public.sync_registry_agent_resource_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.kind = 'agent' THEN
        DELETE FROM registry_agent_resource_refs
        WHERE agent_id = OLD.id AND agent_version = OLD.version;
    END IF;
    IF NEW.kind = 'agent' THEN
        INSERT INTO registry_agent_resource_refs(
            agent_id, agent_version, required_kind, ordinal, reference_id, reference_version
        )
        SELECT NEW.id, NEW.version, 'tool', reference.ordinality::integer,
               reference.value->>'id', reference.value->>'version'
        FROM jsonb_array_elements(COALESCE(NEW.metadata#>'{config,tools}', '[]'::jsonb))
             WITH ORDINALITY AS reference(value, ordinality);
        INSERT INTO registry_agent_resource_refs(
            agent_id, agent_version, required_kind, ordinal, reference_id, reference_version
        )
        SELECT NEW.id, NEW.version, 'skill', reference.ordinality::integer,
               reference.value->>'id', reference.value->>'version'
        FROM jsonb_array_elements(COALESCE(NEW.metadata#>'{config,skills}', '[]'::jsonb))
             WITH ORDINALITY AS reference(value, ordinality);
        IF jsonb_typeof(NEW.metadata#>'{config,cluster}') = 'object' THEN
            INSERT INTO registry_agent_resource_refs(
                agent_id, agent_version, required_kind, ordinal, reference_id, reference_version
            )
            VALUES (
                NEW.id, NEW.version, 'cluster', 1,
                NEW.metadata#>>'{config,cluster,id}',
                NEW.metadata#>>'{config,cluster,version}'
            );
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE FUNCTION public.validate_registry_installations() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Re-run the same effective-config validation when a base registry record
    -- changes; otherwise previously-valid overrides can become invalid.
    IF TG_OP = 'UPDATE' THEN
        UPDATE installations SET config = config
        WHERE id = NEW.id AND version = NEW.version;
    END IF;
    -- Model overrides are independent registry references. Revalidate them
    -- when their target is updated or deleted as well.
    UPDATE installations AS installed SET config = installed.config
    FROM registry AS agent_record
    WHERE agent_record.id = installed.id
      AND agent_record.version = installed.version
      AND agent_record.kind = 'agent'
      AND installed.config#>>'{model,id}' = OLD.id
      AND installed.config#>>'{model,version}' = OLD.version;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;


SET default_tablespace = '';

SET default_table_access_method = heap;
