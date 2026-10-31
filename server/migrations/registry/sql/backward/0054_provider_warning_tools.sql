-- Validation function DDL; do not invalidate immutable warning-tool history.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'tool' AND metadata #>> '{config,provider}' IN (
   'provider.openrouter.advisor@1','openrouter:advisor', 'provider.openrouter.subagent@1','openrouter:subagent',
   'provider.openrouter.shell@1','openrouter:shell', 'provider.openrouter.tool_search@1','openrouter:tool_search',
   'provider.openrouter.experimental__search_models@1','openrouter:experimental__search_models')) THEN
  RAISE EXCEPTION 'registry 0054_provider_warning_tools cannot be reversed: definitions require Provider tools'
   USING ERRCODE = '55000';
 END IF;
 -- Any retained provider pin fences downgrade conservatively, including bundles.
 IF to_regclass('public.runs') IS NOT NULL AND EXISTS(SELECT 1 FROM public.runs WHERE
   jsonb_path_exists(context, 'lax $.binding_snapshot.bindings[*] ? (exists(@.narrow.provider_tool) || @.definition.config.tier == "provider")')
   OR jsonb_path_exists(context, 'lax $.binding_snapshot.definitions[*].definition ? (@.kind == "tool" && @.config.tier == "provider")')) THEN
  RAISE EXCEPTION 'registry 0054_provider_warning_tools cannot be reversed: Runs pin Provider tools' USING ERRCODE = '55000';
 END IF;
END $$;
DO $$
DECLARE original text; copied text; name text; backup text;
BEGIN
 FOREACH name IN ARRAY ARRAY['aidash_provider_tool_binding_is_valid','aidash_descriptor_is_valid'] LOOP
  backup := name || '_before_warning_0051';
  original := pg_get_functiondef(('public.' || backup || '(jsonb)')::regprocedure);
  copied := replace(original, 'FUNCTION public.' || backup || '(', 'FUNCTION public.' || name || '(');
  IF copied = original THEN RAISE EXCEPTION 'contract backup header missing'; END IF;
  EXECUTE copied;
  EXECUTE 'DROP FUNCTION public.' || backup || '(jsonb)';
 END LOOP;
END $$;
