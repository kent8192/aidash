-- PostgreSQL function bodies are DDL and cannot be expressed as SchemaExpr.
DO $migration$
DECLARE original text; extended text;
BEGIN
 original := pg_get_functiondef('public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure);
 extended := replace(original,
  '''agent_plugin_settings'']::text[]',
  '''agent_plugin_settings'',''reference_embedding'']::text[]');
 IF extended = original THEN RAISE EXCEPTION 'Agent config allowlist anchor missing'; END IF;
 original := extended;
 extended := replace(extended,
  ' OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value->''model'' || ''{"registry_deployment":"aidash://contract"}''::jsonb),false)',
  ' OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value->''model'' || ''{"registry_deployment":"aidash://contract"}''::jsonb),false)
 OR (COALESCE(value->''reference_embedding'',''null''::jsonb) <> ''null''::jsonb AND NOT COALESCE(public.aidash_qualified_ref_is_valid(value->''reference_embedding'' || ''{"registry_deployment":"aidash://contract"}''::jsonb),false))');
 IF extended = original THEN RAISE EXCEPTION 'Agent model validation anchor missing'; END IF;
 original := extended;
 extended := replace(extended,
  '''workspace_observe'',''workspace_wait''',
  '''workspace_observe'',''workspace_search'',''workspace_wait''');
 IF extended = original THEN RAISE EXCEPTION 'Agent default tools anchor missing'; END IF;
 EXECUTE extended;
END $migration$;
