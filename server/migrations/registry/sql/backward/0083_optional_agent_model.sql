-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL.
DO $$ BEGIN
 IF EXISTS (SELECT 1 FROM registry WHERE kind = 'agent' AND metadata #>> '{config,model}' IS NULL) THEN
  RAISE EXCEPTION 'Agent versions still need Task Models' USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; edited text;
BEGIN
 original := pg_get_functiondef('public.aidash_agent_before_provider_tools_0050(jsonb)'::regprocedure);
 edited := replace(original, $before$(COALESCE(value->'model','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_deployment":"aidash://contract"}'::jsonb),false))$before$, $after$NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_deployment":"aidash://contract"}'::jsonb),false)$after$);
 IF edited = original THEN RAISE EXCEPTION 'unexpected aidash_agent_before_provider_tools_0050(jsonb) body'; END IF;
 EXECUTE edited;
END $$;
DO $$
DECLARE original text; edited text;
BEGIN
 original := pg_get_functiondef('public.sync_registry_agent_model_refs()'::regprocedure);
 edited := replace(original, $before$IF NEW.kind = 'agent' AND NEW.metadata #>> '{config,model}' IS NOT NULL THEN$before$, $after$IF NEW.kind = 'agent' THEN$after$);
 IF edited = original THEN RAISE EXCEPTION 'unexpected sync_registry_agent_model_refs() body'; END IF;
 EXECUTE edited;
END $$;
