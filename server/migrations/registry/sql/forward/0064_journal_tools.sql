-- PostgreSQL validation functions require SQL DDL. Preserve all prior rules.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_before_journal_0064(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'Journal descriptor contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_before_journal_0064(');
 IF copied = original THEN RAISE EXCEPTION 'Descriptor contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'operation' IN ('journal_search', 'journal_read', 'journal_query') THEN
  IF value->>'provider' IS DISTINCT FROM 'core.journal@1' THEN RETURN false; END IF;
  RETURN public.aidash_descriptor_before_journal_0064(value || '{"provider":"core.exposure@1","operation":"capability_search"}'::jsonb);
 END IF;
 RETURN public.aidash_descriptor_before_journal_0064(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
