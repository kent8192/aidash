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

-- The built-in descriptor is part of the same retrieval capability.
DO $migration$
DECLARE original text; extended text;
BEGIN
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 extended := replace(original,
  'WHEN ''workspace_wait'' THEN ''core.workspace@1''',
  'WHEN ''workspace_search'' THEN ''core.workspace@1'' WHEN ''workspace_wait'' THEN ''core.workspace@1''');
 IF extended = original THEN RAISE EXCEPTION 'Workspace descriptor anchor missing'; END IF;
 EXECUTE extended;
END $migration$;

-- Match the owning knowledge row's Tenant isolation and transaction guard.
ALTER TABLE public.agent_knowledge_images ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.agent_knowledge_images FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.agent_knowledge_images AS PERMISSIVE FOR ALL TO PUBLIC
 USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
 WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.agent_knowledge_images FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
