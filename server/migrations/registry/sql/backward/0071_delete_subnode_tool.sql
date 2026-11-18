-- Validation function DDL; do not invalidate stored tool descriptors.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,operation}' = 'delete_subnode') THEN
  RAISE EXCEPTION 'registry 0071_delete_subnode_tool cannot be reversed: definitions declare delete_subnode'
   USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_is_valid_before_delete_subnode_0071(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid_before_delete_subnode_0071(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_descriptor_is_valid_before_delete_subnode_0071(jsonb);
END $$;
