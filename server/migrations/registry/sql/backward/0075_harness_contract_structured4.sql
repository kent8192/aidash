-- Validation function DDL; do not invalidate stored Agents or pinned Runs.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'agent' AND metadata #>> '{config,harness_contract}' = 'structured@4') THEN
  RAISE EXCEPTION 'registry 0075_harness_contract_structured4 cannot be reversed: definitions declare structured@4'
   USING ERRCODE = '55000';
 END IF;
 IF to_regclass('public.marketplace_versions') IS NOT NULL THEN
  IF EXISTS(SELECT 1 FROM public.marketplace_versions WHERE kind = 'agent'
   AND (document->>'manifest_source')::jsonb #>> '{entity,config,harness_contract}' = 'structured@4') THEN
   RAISE EXCEPTION 'registry 0075_harness_contract_structured4 cannot be reversed: packages declare structured@4'
    USING ERRCODE = '55000';
  END IF;
 END IF;
 -- Nest the query so PostgreSQL does not plan it when the table is absent.
 IF to_regclass('public.runs') IS NOT NULL THEN
  IF EXISTS(SELECT 1 FROM public.runs WHERE
   jsonb_path_exists(context, 'lax $.binding_snapshot.definitions[*].definition ? (@.kind == "agent" && @.config.harness_contract == "structured@4")')
   OR jsonb_path_exists(context, 'lax $.binding_snapshot.foreign_agents[*].definitions[*].definition ? (@.kind == "agent" && @.config.harness_contract == "structured@4")')) THEN
   RAISE EXCEPTION 'registry 0075_harness_contract_structured4 cannot be reversed: Runs pin Agent definitions declaring structured@4'
    USING ERRCODE = '55000';
  END IF;
 END IF;
END $$;
DO $$
DECLARE original text; copied text;
BEGIN
 original := pg_get_functiondef('public.aidash_agent_before_structured4_0075(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_agent_before_structured4_0075(', 'FUNCTION public.aidash_agent_bindings_is_valid(');
 IF copied = original THEN RAISE EXCEPTION 'Agent contract backup header missing'; END IF;
 EXECUTE copied;
 DROP FUNCTION public.aidash_agent_before_structured4_0075(jsonb);
END $$;
