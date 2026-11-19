-- Validation function DDL; do not invalidate stored Node tool descriptors.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' = 'core.nodes@1') THEN
  RAISE EXCEPTION 'registry 0080_node_tools cannot be reversed: definitions declare Node tools'
   USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_is_valid_before_nodes_0080(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid_before_nodes_0080(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_descriptor_is_valid_before_nodes_0080(jsonb);
END $$;
