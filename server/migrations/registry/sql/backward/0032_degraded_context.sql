-- Restore the 0011_binding_contract context validator.
-- Rollback precondition: no source Definition or package entry declares
-- `on_unavailable`. Earlier binaries decode source configs with
-- deny_unknown_fields, so this read-only guard refuses the rollback instead of
-- leaving records no binary can read.
DO $$
DECLARE definition_count bigint; package_count bigint;
BEGIN
  SELECT count(*) INTO definition_count FROM registry
   WHERE kind = 'source' AND metadata #> '{config,source}' ? 'on_unavailable';
  -- Marketplace packages may live in another database.
  package_count := 0;
  IF to_regclass('packages') IS NOT NULL THEN
    EXECUTE $q$SELECT count(*) FROM packages WHERE jsonb_path_exists(manifest, 'lax $.**.on_unavailable')$q$
      INTO package_count;
  END IF;
  IF definition_count + package_count > 0 THEN
    RAISE EXCEPTION 'registry 0032_degraded_context cannot be reversed: % Definitions and % packages declare on_unavailable', definition_count, package_count
      USING ERRCODE = '55000',
            HINT = 'Earlier binaries cannot read these records; remove on_unavailable from them or restore the pre-upgrade database backup instead of reversing this migration.';
  END IF;
END $$;
CREATE OR REPLACE FUNCTION public.aidash_context_is_valid(value jsonb, kind text) RETURNS boolean
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
