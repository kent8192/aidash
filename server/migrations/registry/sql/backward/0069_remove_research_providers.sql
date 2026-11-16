-- Restores the descriptor contract that admitted every research provider. It
-- accepts everything the narrower one did. The removed draft Bindings pointed
-- at definitions that no longer serve anything; they are not restored.
DO $$
DECLARE original text; restored text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_before_research_0069(jsonb)'::regprocedure);
 restored := replace(original, 'FUNCTION public.aidash_descriptor_before_research_0069(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF restored = original THEN RAISE EXCEPTION 'Descriptor contract backup header missing'; END IF;
 EXECUTE restored;
END $$;
DROP FUNCTION public.aidash_descriptor_before_research_0069(jsonb);
