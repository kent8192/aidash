-- PostgreSQL function bodies have no typed Reinhardt migration operation. DDL only.
-- Refuse a downgrade while immutable definitions or retained Run snapshots need
-- the new capability: earlier binaries cannot read them safely.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE
   (kind = 'tool' AND metadata #>> '{config,tier}' = 'provider')
   OR (kind = 'agent' AND jsonb_path_exists(metadata, 'lax $.config.bindings[*].narrow.provider_tool'))) THEN
  RAISE EXCEPTION 'registry 0053_provider_tools cannot be reversed: definitions require Provider tools'
   USING ERRCODE = '55000', HINT = 'Restore the pre-upgrade database backup instead of discarding immutable capability history.';
 END IF;
 IF to_regclass('public.runs') IS NOT NULL THEN
  IF EXISTS(SELECT 1 FROM public.runs WHERE
   jsonb_path_exists(context, 'lax $.binding_snapshot.bindings[*] ? (exists(@.narrow.provider_tool) || @.definition.config.tier == "provider")')
   OR jsonb_path_exists(context, 'lax $.binding_snapshot.definitions[*].definition ? (@.kind == "tool" && @.config.tier == "provider")')
   OR jsonb_path_exists(context, 'lax $.binding_snapshot.definitions[*].definition.config.bindings[*].narrow.provider_tool')) THEN
   RAISE EXCEPTION 'registry 0053_provider_tools cannot be reversed: Runs pin Provider tools'
    USING ERRCODE = '55000', HINT = 'Restore the pre-upgrade database backup instead of discarding retained Run capability history.';
  END IF;
 END IF;
END $$;
DO $$
DECLARE original text; restored text;
BEGIN
 original := pg_get_functiondef('public.aidash_agent_before_provider_tools_0050(jsonb)'::regprocedure);
 restored := replace(original, 'FUNCTION public.aidash_agent_before_provider_tools_0050(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
 IF restored = original THEN RAISE EXCEPTION 'Agent contract backup header missing'; END IF;
 EXECUTE restored;
 original := pg_get_functiondef('public.aidash_descriptor_before_provider_tools_0050(jsonb)'::regprocedure);
 restored := replace(original, 'FUNCTION public.aidash_descriptor_before_provider_tools_0050(', 'FUNCTION public.aidash_descriptor_is_valid(');
 IF restored = original THEN RAISE EXCEPTION 'Descriptor contract backup header missing'; END IF;
 EXECUTE restored;
END $$;
DROP FUNCTION public.aidash_agent_before_provider_tools_0050(jsonb);
DROP FUNCTION public.aidash_descriptor_before_provider_tools_0050(jsonb);
DROP FUNCTION public.aidash_provider_tool_binding_is_valid(jsonb);
