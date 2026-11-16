-- Refuse to invalidate retained immutable Journal definitions on downgrade.
DO $$
DECLARE original text; restored text;
BEGIN
 IF EXISTS (SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' = 'core.journal@1') THEN
  RAISE EXCEPTION 'registry 0064_journal_tools cannot be reversed: definitions require Journal tools' USING ERRCODE = '55000';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_before_journal_0064(jsonb)'::regprocedure);
 restored := replace(original, 'FUNCTION public.aidash_descriptor_before_journal_0064(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF restored = original THEN RAISE EXCEPTION 'Descriptor contract backup header missing'; END IF;
 EXECUTE restored;
END $$;
DROP FUNCTION public.aidash_descriptor_before_journal_0064(jsonb);
