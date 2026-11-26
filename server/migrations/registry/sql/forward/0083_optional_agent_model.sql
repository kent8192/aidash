-- PostgreSQL function bodies cannot be expressed by typed Reinhardt DDL.
DO $$
DECLARE original text; edited text;
BEGIN
 original := pg_get_functiondef('public.aidash_agent_before_provider_tools_0050(jsonb)'::regprocedure);
 edited := replace(original, $before$NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_deployment":"aidash://contract"}'::jsonb),false)$before$, $after$(COALESCE(value->'model','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_deployment":"aidash://contract"}'::jsonb),false))$after$);
 IF edited = original THEN RAISE EXCEPTION 'unexpected aidash_agent_before_provider_tools_0050(jsonb) body'; END IF;
 EXECUTE edited;
END $$;
DO $$
DECLARE original text; edited text;
BEGIN
 original := pg_get_functiondef('public.sync_registry_agent_model_refs()'::regprocedure);
 edited := replace(original, $before$IF NEW.kind = 'agent' THEN$before$, $after$IF NEW.kind = 'agent' AND NEW.metadata #>> '{config,model}' IS NOT NULL THEN$after$);
 IF edited = original THEN RAISE EXCEPTION 'unexpected sync_registry_agent_model_refs() body'; END IF;
 EXECUTE edited;
END $$;
