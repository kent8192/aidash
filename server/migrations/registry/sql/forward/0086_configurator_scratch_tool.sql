-- PostgreSQL validation function DDL has no typed Reinhardt operation.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_configurator_scratch_0086(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'configurator tool contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_configurator_scratch_0086(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' = 'core.configurator@1' AND value->>'operation' = 'scratch_space_configure'
    AND value->>'default_alias' = 'scratch_space_configure' THEN
  value := value || jsonb_build_object('operation', 'catalog_approve', 'default_alias', 'catalog_approve');
 END IF;
 RETURN public.aidash_descriptor_is_valid_before_configurator_scratch_0086(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
