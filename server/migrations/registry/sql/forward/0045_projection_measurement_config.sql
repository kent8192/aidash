-- DDL exception: catalog-derived JSONB constraints and PL/pgSQL validation
-- functions have no typed Reinhardt schema equivalent. No application DML.
DO $$
DECLARE definition text; edited text;
BEGIN
  SELECT pg_get_constraintdef(oid) INTO STRICT definition
    FROM pg_constraint WHERE conrelid = 'registry'::regclass AND conname = 'registry_model_config';
  edited := replace(definition, '''request_estimator''::text',
                    '''request_estimator''::text, ''provider_route''::text, ''estimate_diagnostics''::text');
  IF edited = definition THEN RAISE EXCEPTION 'Projection measurement allowlist anchor missing'; END IF;
  ALTER TABLE registry DROP CONSTRAINT registry_model_config;
  EXECUTE format('ALTER TABLE registry ADD CONSTRAINT registry_model_config %s', edited);
END $$;

CREATE FUNCTION public.aidash_provider_route_valid(route jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
  IF jsonb_typeof(route) <> 'object'
     OR NOT route ?& ARRAY['tag','source','verified_at','expires_at']::text[]
     OR route - ARRAY['tag','source','verified_at','expires_at']::text[] <> '{}'::jsonb
     OR jsonb_typeof(route->'tag') <> 'string'
     OR length(route->>'tag') NOT BETWEEN 1 AND 256
     OR route->>'tag' !~ '^[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)*$'
     OR route->>'tag' ~ '(^|/)[.]{1,2}(/|$)'
     OR jsonb_typeof(route->'source') <> 'string'
     OR length(route->>'source') NOT BETWEEN 1 AND 1024
     OR route->>'source' ~ '[[:cntrl:]]'
  THEN RETURN false; END IF;
  -- Reuse the established exact timestamp and provenance validation.
  RETURN COALESCE(public.aidash_media_routes_valid(
      jsonb_build_array(route || '{"formats":["image/png"]}'::jsonb)), false);
END $$;

CREATE FUNCTION public.aidash_estimate_diagnostics_valid(config jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE projection text; calibration jsonb;
BEGIN
  IF jsonb_typeof(config) <> 'object'
     OR config - ARRAY['token_encoding','calibrations']::text[] <> '{}'::jsonb
     OR (config ? 'token_encoding' AND config->'token_encoding' NOT IN
         ('null'::jsonb, '"o200k_base"'::jsonb, '"gemma4"'::jsonb))
  THEN RETURN false; END IF;
  IF NOT config ? 'calibrations' THEN RETURN true; END IF;
  IF jsonb_typeof(config->'calibrations') <> 'object' THEN RETURN false; END IF;
  FOR projection, calibration IN SELECT key, value FROM jsonb_each(config->'calibrations') LOOP
    IF projection NOT IN ('legacy','ordered','native') OR jsonb_typeof(calibration) <> 'object'
       OR NOT calibration ?& ARRAY['ratio_ppm','samples']::text[]
       OR calibration - ARRAY['ratio_ppm','samples']::text[] <> '{}'::jsonb
       OR jsonb_typeof(calibration->'ratio_ppm') <> 'number'
       OR jsonb_typeof(calibration->'samples') <> 'number'
       OR (calibration->'ratio_ppm')::text !~ '^[0-9]+$'
       OR (calibration->'samples')::text !~ '^[0-9]+$'
    THEN RETURN false; END IF;
    IF (calibration->>'ratio_ppm')::numeric NOT BETWEEN 1 AND 1000000
       OR (calibration->>'samples')::numeric NOT BETWEEN 0 AND 200
    THEN RETURN false; END IF;
  END LOOP;
  RETURN true;
END $$;

ALTER TABLE registry ADD CONSTRAINT registry_model_projection_measurement CHECK (
  kind <> 'model' OR COALESCE(
    (NOT metadata->'config' ? 'provider_route'
      OR metadata #> '{config,provider_route}' = 'null'::jsonb
      OR ((metadata #>> '{config,provider}') = 'openrouter'
        AND COALESCE(metadata #> '{config,plan_connection}', 'null'::jsonb) = 'null'::jsonb
        AND public.aidash_provider_route_valid(metadata #> '{config,provider_route}')))
    AND (NOT metadata->'config' ? 'estimate_diagnostics'
      OR metadata #> '{config,estimate_diagnostics}' = 'null'::jsonb
      OR public.aidash_estimate_diagnostics_valid(metadata #> '{config,estimate_diagnostics}')),
    false)
);
