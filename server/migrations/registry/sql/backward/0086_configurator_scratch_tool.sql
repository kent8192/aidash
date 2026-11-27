-- Validation function DDL; do not invalidate stored Configurator tool descriptors.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' = 'core.configurator@1' AND metadata #>> '{config,operation}' = 'scratch_space_configure') THEN
  RAISE EXCEPTION 'registry 0086_configurator_scratch_tool cannot be reversed: definitions declare Configurator tools'
   USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_is_valid_before_configurator_scratch_0086(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid_before_configurator_scratch_0086(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_descriptor_is_valid_before_configurator_scratch_0086(jsonb);
END $$;
