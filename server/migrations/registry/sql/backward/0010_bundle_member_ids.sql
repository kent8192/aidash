-- Restore the previous exact-reference validation without changing stored bytes.
CREATE OR REPLACE FUNCTION public.aidash_bundle_is_valid(value jsonb) RETURNS boolean
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
