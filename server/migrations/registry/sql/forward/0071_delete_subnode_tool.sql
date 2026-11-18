-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL operations.
-- Admit the `delete_subnode` builtin (#1012), the renamed `subnode_dissolve`,
-- with exactly the rules of the operation it replaces; retain the previous
-- validator.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_delete_subnode_0071(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'delete_subnode contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_delete_subnode_0071(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF value->>'operation' = 'delete_subnode' THEN
  IF value->>'default_alias' IS DISTINCT FROM 'delete_subnode' THEN RETURN false; END IF;
  value := value || '{"operation":"subnode_dissolve","default_alias":"subnode_dissolve"}'::jsonb;
 END IF;
 RETURN public.aidash_descriptor_is_valid_before_delete_subnode_0071(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
