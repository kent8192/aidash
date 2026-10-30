-- Never silently remove a private collection or invalidate a pinned Agent/Run.
DO $migration$
DECLARE runs_in_use boolean := false;
BEGIN
 IF to_regclass('public.runs') IS NOT NULL THEN
  EXECUTE 'SELECT EXISTS (SELECT 1 FROM runs WHERE context::text LIKE ''%aidash.workspace_search%'' OR context::text LIKE ''%aidash.workspace_read%1.1.0%'')' INTO runs_in_use;
 END IF;
 IF EXISTS (SELECT 1 FROM agent_knowledge WHERE image_index IS NOT NULL OR image_data IS NOT NULL)
 OR EXISTS (SELECT 1 FROM registry WHERE kind = 'agent' AND
   (metadata::text LIKE '%"reference_embedding"%' OR metadata::text LIKE '%aidash.workspace_search%' OR metadata::text LIKE '%aidash.workspace_read%1.1.0%'))
 OR EXISTS (SELECT 1 FROM agent_drafts WHERE entry::text LIKE '%"reference_embedding"%' OR entry::text LIKE '%aidash.workspace_search%')
 OR runs_in_use THEN
  RAISE EXCEPTION 'reference image data or pinned image retrieval is in use; rollback requires a compatible version';
 END IF;
END $migration$;
-- PostgreSQL function bodies have no typed migration operation. DDL only.
-- Agent Bindings accept what an Agent version decides about the servers of the
-- Agent Plugins it takes in (#414, ADR 0072). The body keeps every key of
-- 0042_external_services and adds `agent_plugin_settings`: an object
-- whose members are objects (plugin name to server name to settings). A
-- settings object holds only `tools` (an array of strings, or null), `exposure`
-- (`eager` or `deferred`, or null) and `read_only` (an array of objects with a
-- string `name` and a string `schema_digest`). Domain validation checks the
-- rest: that each key names an attached plugin and server, the name and count
-- limits, and the digest format. An explicit null is rejected for the key
-- itself and for `read_only`, because the Rust fields have no Option there.
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE plugins jsonb; plugin jsonb; plugin_settings jsonb; plugin_choices jsonb; server_choice jsonb; declared jsonb; completion jsonb; check_item jsonb; item jsonb; removals jsonb; edges jsonb; step_count numeric; parallelism numeric; context_policy jsonb; summary jsonb; restriction jsonb; candidate jsonb; policy jsonb; deferred boolean; budget text; bound numeric; removable text[]; support text[];
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR value - ARRAY['schema_version','model','instructions','bindings','remove_default','cluster','max_steps','projection_version','prompt_cache','exposure','tool_parallelism','context_policy','completion','agent_plugins','agent_plugin_settings']::text[] <> '{}'::jsonb
 OR NOT COALESCE(value->'projection_version','null'::jsonb) IN ('null'::jsonb,'"legacy"'::jsonb,'"ordered"'::jsonb,'"native"'::jsonb)
 OR NOT COALESCE(value->'prompt_cache','null'::jsonb) IN ('null'::jsonb,'"off"'::jsonb,'"explicit"'::jsonb)
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb
 OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_deployment":"aidash://contract"}'::jsonb),false)
 OR jsonb_typeof(COALESCE(value->'instructions','""'::jsonb)) <> 'string' THEN RETURN false; END IF;
 IF COALESCE(value->'cluster','null'::jsonb) <> 'null'::jsonb
 AND NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'cluster' || '{"registry_deployment":"aidash://contract"}'::jsonb),false) THEN RETURN false; END IF;
 IF jsonb_typeof(COALESCE(value->'max_steps','64'::jsonb)) <> 'number' OR NOT COALESCE(COALESCE(value->>'max_steps','64') ~ '^[0-9]+$',false) THEN RETURN false; END IF;
 step_count := COALESCE(value->>'max_steps','64')::numeric;
 IF step_count NOT BETWEEN 1 AND 1000 THEN RETURN false; END IF;
 IF jsonb_typeof(COALESCE(value->'tool_parallelism','1'::jsonb)) <> 'number' OR NOT COALESCE(COALESCE(value->>'tool_parallelism','1') ~ '^[0-9]+$',false) THEN RETURN false; END IF;
 parallelism := COALESCE(value->>'tool_parallelism','1')::numeric;
 IF parallelism NOT BETWEEN 1 AND 16 THEN RETURN false; END IF;
 -- Context Policy mirrors ContextPolicy::validate: context-recovery/1 only,
 -- preserve_recent 6..=1000, overflow_retries 0..=4, overflow_shrink
 -- 0.5..=0.9 and an optional summary with an exact model, max_tokens
 -- 256..=4096 and call_budget 1..=64. Omitted fields take the Rust defaults;
 -- an explicit null is accepted only where the Rust type is optional.
 context_policy := COALESCE(value->'context_policy','null'::jsonb);
 IF context_policy <> 'null'::jsonb THEN
  IF jsonb_typeof(context_policy) <> 'object'
  OR context_policy - ARRAY['version','preserve_recent','overflow_retries','overflow_shrink','summary']::text[] <> '{}'::jsonb
  OR context_policy->'version' IS DISTINCT FROM '"context-recovery/1"'::jsonb THEN RETURN false; END IF;
  IF jsonb_typeof(COALESCE(context_policy->'preserve_recent','6'::jsonb)) <> 'number' OR NOT COALESCE(context_policy->>'preserve_recent','6') ~ '^[0-9]+$'
  OR jsonb_typeof(COALESCE(context_policy->'overflow_retries','2'::jsonb)) <> 'number' OR NOT COALESCE(context_policy->>'overflow_retries','2') ~ '^[0-9]+$'
  OR jsonb_typeof(COALESCE(context_policy->'overflow_shrink','0.75'::jsonb)) <> 'number' THEN RETURN false; END IF;
  IF COALESCE(context_policy->>'preserve_recent','6')::numeric NOT BETWEEN 6 AND 1000
  OR COALESCE(context_policy->>'overflow_retries','2')::numeric > 4
  OR COALESCE(context_policy->>'overflow_shrink','0.75')::numeric NOT BETWEEN 0.5 AND 0.9 THEN RETURN false; END IF;
  summary := COALESCE(context_policy->'summary','null'::jsonb);
  IF summary <> 'null'::jsonb THEN
   IF jsonb_typeof(summary) <> 'object' OR summary - ARRAY['model','max_tokens','call_budget']::text[] <> '{}'::jsonb
   OR NOT COALESCE(public.aidash_qualified_ref_is_valid(summary->'model' || '{"registry_deployment":"aidash://contract"}'::jsonb),false)
   OR jsonb_typeof(COALESCE(summary->'max_tokens','4096'::jsonb)) <> 'number' OR NOT COALESCE(summary->>'max_tokens','4096') ~ '^[0-9]+$'
   OR jsonb_typeof(COALESCE(summary->'call_budget','8'::jsonb)) <> 'number' OR NOT COALESCE(summary->>'call_budget','8') ~ '^[0-9]+$' THEN RETURN false; END IF;
   IF COALESCE(summary->>'max_tokens','4096')::numeric NOT BETWEEN 256 AND 4096
   OR COALESCE(summary->>'call_budget','8')::numeric NOT BETWEEN 1 AND 64 THEN RETURN false; END IF;
  END IF;
 END IF;
 -- Completion Policy mirrors CompletionPolicy::validate (#112): checks/1
 -- only, 1 to 8 Checks with distinct lowercase names, capability "shell", a
 -- non-blank command of at most 65536 bytes, timeout_seconds 1..=600 or null,
 -- when always or content_changed, repair_allowance 0..=16,
 -- verification_seconds 1..=3600 and a boolean exception.
 completion := COALESCE(value->'completion','null'::jsonb);
 IF completion <> 'null'::jsonb THEN
  IF jsonb_typeof(completion) <> 'object'
  OR completion - ARRAY['version','checks','repair_allowance','verification_seconds','exception']::text[] <> '{}'::jsonb
  OR completion->'version' IS DISTINCT FROM '"checks/1"'::jsonb
  OR jsonb_typeof(completion->'checks') IS DISTINCT FROM 'array'
  OR jsonb_array_length(completion->'checks') NOT BETWEEN 1 AND 8
  OR jsonb_typeof(COALESCE(completion->'repair_allowance','3'::jsonb)) <> 'number' OR NOT COALESCE(completion->>'repair_allowance','3') ~ '^[0-9]+$'
  OR jsonb_typeof(COALESCE(completion->'verification_seconds','1800'::jsonb)) <> 'number' OR NOT COALESCE(completion->>'verification_seconds','1800') ~ '^[0-9]+$'
  OR jsonb_typeof(COALESCE(completion->'exception','false'::jsonb)) <> 'boolean' THEN RETURN false; END IF;
  IF COALESCE(completion->>'repair_allowance','3')::numeric > 16
  OR COALESCE(completion->>'verification_seconds','1800')::numeric NOT BETWEEN 1 AND 3600 THEN RETURN false; END IF;
  FOR check_item IN SELECT jsonb_array_elements(completion->'checks') LOOP
   IF jsonb_typeof(check_item) <> 'object'
   OR check_item - ARRAY['name','capability','command','timeout_seconds','when']::text[] <> '{}'::jsonb
   OR jsonb_typeof(check_item->'name') IS DISTINCT FROM 'string'
   OR NOT COALESCE(check_item->>'name' ~ '^[a-z0-9][a-z0-9_-]{0,63}$',false)
   OR check_item->'capability' IS DISTINCT FROM '"shell"'::jsonb
   OR jsonb_typeof(check_item->'command') IS DISTINCT FROM 'string'
   OR length(btrim(check_item->>'command', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) = 0
   OR octet_length(check_item->>'command') > 65536
   OR NOT COALESCE(check_item->'when','"content_changed"'::jsonb) IN ('"always"'::jsonb,'"content_changed"'::jsonb) THEN RETURN false; END IF;
   IF COALESCE(check_item->'timeout_seconds','null'::jsonb) <> 'null'::jsonb THEN
    IF jsonb_typeof(check_item->'timeout_seconds') <> 'number' OR NOT (check_item->>'timeout_seconds') ~ '^[0-9]+$'
    OR (check_item->>'timeout_seconds')::numeric NOT BETWEEN 1 AND 600 THEN RETURN false; END IF;
   END IF;
  END LOOP;
  IF (SELECT count(*) <> count(DISTINCT e->'name') FROM jsonb_array_elements(completion->'checks') e) THEN RETURN false; END IF;
 END IF;
 -- Agent Plugins mirror AgentBindings::validate_agent_plugins and the shape
 -- of AgentPluginRef: at most 8 references, each pinned by a sha256 digest.
 plugins := COALESCE(value->'agent_plugins','[]'::jsonb);
 IF jsonb_typeof(plugins) <> 'array' OR jsonb_array_length(plugins) > 8 THEN RETURN false; END IF;
 FOR plugin IN SELECT jsonb_array_elements(plugins) LOOP
  IF jsonb_typeof(plugin) <> 'object'
  OR jsonb_typeof(plugin->'digest') IS DISTINCT FROM 'string'
  OR NOT COALESCE(plugin->>'digest' ~ '^sha256:[0-9a-f]{64}$',false)
  OR jsonb_typeof(plugin->'name') IS DISTINCT FROM 'string'
  OR jsonb_typeof(plugin->'source') IS DISTINCT FROM 'string'
  OR jsonb_typeof(plugin->'skills') IS DISTINCT FROM 'array'
  OR jsonb_typeof(plugin->'findings') IS DISTINCT FROM 'array'
  OR jsonb_typeof(plugin->'servers') IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 END LOOP;
 -- Agent Plugin settings mirror PluginSettings: plugin name, then server name,
 -- then the choices for that server.
 IF value ? 'agent_plugin_settings' THEN
  plugin_settings := value->'agent_plugin_settings';
  IF jsonb_typeof(plugin_settings) <> 'object' THEN RETURN false; END IF;
  FOR plugin_choices IN SELECT v FROM jsonb_each(plugin_settings) AS plugin_entries(k,v) LOOP
   IF jsonb_typeof(plugin_choices) <> 'object' THEN RETURN false; END IF;
   FOR server_choice IN SELECT v FROM jsonb_each(plugin_choices) AS server_entries(k,v) LOOP
    IF jsonb_typeof(server_choice) <> 'object'
    OR server_choice - ARRAY['tools','exposure','read_only']::text[] <> '{}'::jsonb THEN RETURN false; END IF;
    IF COALESCE(server_choice->'tools','null'::jsonb) <> 'null'::jsonb
    AND (jsonb_typeof(server_choice->'tools') <> 'array'
     OR jsonb_path_exists(server_choice->'tools','strict $[*] ? (@.type() != "string")')) THEN RETURN false; END IF;
    IF NOT COALESCE(server_choice->'exposure','null'::jsonb) IN ('null'::jsonb,'"eager"'::jsonb,'"deferred"'::jsonb) THEN RETURN false; END IF;
    IF server_choice ? 'read_only' THEN
     IF jsonb_typeof(server_choice->'read_only') <> 'array' THEN RETURN false; END IF;
     FOR declared IN SELECT jsonb_array_elements(server_choice->'read_only') LOOP
      IF jsonb_typeof(declared) <> 'object'
      OR declared - ARRAY['name','schema_digest']::text[] <> '{}'::jsonb
      OR jsonb_typeof(declared->'name') IS DISTINCT FROM 'string'
      OR jsonb_typeof(declared->'schema_digest') IS DISTINCT FROM 'string' THEN RETURN false; END IF;
     END LOOP;
    END IF;
   END LOOP;
  END LOOP;
 END IF;
 edges := COALESCE(value->'bindings','[]'::jsonb);
 removals := COALESCE(value->'remove_default','[]'::jsonb);
 IF jsonb_typeof(edges) <> 'array' OR jsonb_typeof(removals) <> 'array' THEN RETURN false; END IF;
 IF jsonb_array_length(edges) > 128 THEN RETURN false; END IF;
 policy := COALESCE(value->'exposure','null'::jsonb);
 deferred := policy->>'version' = 'deferred@1';
 IF policy <> 'null'::jsonb THEN
  IF jsonb_typeof(policy) <> 'object' THEN RETURN false; END IF;
  IF policy->>'version' = 'legacy@1' THEN
   IF policy - 'version' <> '{}'::jsonb THEN RETURN false; END IF;
  ELSIF deferred THEN
   IF policy - ARRAY['version','metadata_bytes','schema_bytes','skill_bytes']::text[] <> '{}'::jsonb THEN RETURN false; END IF;
   FOREACH budget IN ARRAY ARRAY['metadata_bytes','schema_bytes','skill_bytes'] LOOP
    IF policy ? budget AND jsonb_typeof(policy->budget) = 'null' THEN RETURN false; END IF;
    IF COALESCE(policy->budget,'null'::jsonb) <> 'null'::jsonb THEN
     IF jsonb_typeof(policy->budget) <> 'number' OR NOT (policy->>budget) ~ '^[0-9]+$' THEN RETURN false; END IF;
     bound := (policy->>budget)::numeric;
     IF budget = 'metadata_bytes' AND bound NOT BETWEEN 512 AND 65536
     OR budget <> 'metadata_bytes' AND bound NOT BETWEEN 1024 AND 262144 THEN RETURN false; END IF;
    END IF;
   END LOOP;
  ELSE RETURN false; END IF;
 END IF;
 deferred := COALESCE(deferred,false);
 -- plpgsql ends an IF condition at its first THEN, so no CASE inside one.
 IF deferred THEN
  removable := ARRAY['workspace_observe','workspace_wait','skill_asset_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','memory_mutate','memory_recall','memory_reflect','compaction'];
  support := ARRAY['skill_asset_read'];
 ELSE
  removable := ARRAY['workspace_observe','workspace_wait','skill_list','skill_load','skill_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','memory_mutate','memory_recall','memory_reflect','compaction'];
  support := ARRAY['skill_list','skill_load','skill_read'];
 END IF;
 FOR item IN SELECT jsonb_array_elements(edges) LOOP
  IF jsonb_typeof(item) <> 'object' OR item - ARRAY['kind','target','alias','narrow','members','exposure']::text[] <> '{}'::jsonb
  OR COALESCE(item->'exposure','null'::jsonb) <> 'null'::jsonb
  AND NOT (deferred AND item->>'kind' IN ('tool','bundle','skill') AND COALESCE(item->>'exposure' IN ('eager','deferred'),false))
  OR NOT COALESCE(item->>'kind' IN ('tool','bundle','skill','memory','source','external_service','decider'),false)
  OR NOT COALESCE(public.aidash_qualified_ref_is_valid(item->'target'),false)
  OR jsonb_typeof(COALESCE(item->'narrow','{}'::jsonb)) <> 'object'
  OR COALESCE(item->'narrow','{}'::jsonb) - ARRAY['allowed_hosts','scope','limits','concurrency','decision']::text[] <> '{}'::jsonb THEN RETURN false; END IF;
  restriction := COALESCE(item->'narrow','{}'::jsonb);
  -- Current undeployed Decider Binding contract (#103/#108). Approval and
  -- execution still validate exact version ceilings in the application.
  IF COALESCE(restriction->'decision','null'::jsonb) <> 'null'::jsonb THEN
   candidate := restriction->'decision';
   IF item->>'kind' <> 'decider' OR jsonb_typeof(candidate) <> 'object'
   OR candidate - ARRAY['threshold','preserve_recent','forbid_apply','max_calls_per_run']::text[] <> '{}'::jsonb
   OR jsonb_typeof(COALESCE(candidate->'threshold','"3fe0000000000000"'::jsonb)) <> 'string'
   OR NOT COALESCE(candidate->>'threshold','3fe0000000000000') ~ '^[0-9a-f]{16}$'
   OR (COALESCE(candidate->>'threshold','3fe0000000000000') COLLATE "C" > '3fe0000000000000'
    AND candidate->>'threshold' <> '8000000000000000')
   OR jsonb_typeof(COALESCE(candidate->'preserve_recent','6'::jsonb)) <> 'number'
   OR NOT COALESCE(candidate->>'preserve_recent','6') ~ '^[0-9]+$'
   OR COALESCE(candidate->>'preserve_recent','6')::numeric < 6
   OR jsonb_typeof(COALESCE(candidate->'forbid_apply','false'::jsonb)) <> 'boolean' THEN RETURN false; END IF;
   IF COALESCE(candidate->'max_calls_per_run','null'::jsonb) <> 'null'::jsonb
   AND (jsonb_typeof(candidate->'max_calls_per_run') <> 'number'
    OR NOT (candidate->>'max_calls_per_run') ~ '^[1-9][0-9]*$'
    OR (candidate->>'max_calls_per_run')::numeric > 18446744073709551615) THEN RETURN false; END IF;
  END IF;
  IF item->>'kind' = 'decider' AND (
   COALESCE(restriction->'allowed_hosts','null'::jsonb) <> 'null'::jsonb
   OR COALESCE(restriction->'scope','{}'::jsonb) <> '{}'::jsonb
   OR COALESCE(restriction->'limits','{}'::jsonb) <> '{}'::jsonb) THEN RETURN false; END IF;
  IF COALESCE(restriction->'allowed_hosts','null'::jsonb) <> 'null'::jsonb THEN
   IF jsonb_typeof(restriction->'allowed_hosts') <> 'array' OR jsonb_array_length(restriction->'allowed_hosts') = 0
   OR jsonb_path_exists(restriction->'allowed_hosts','strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
  END IF;
  IF COALESCE(restriction->'concurrency','null'::jsonb) <> 'null'::jsonb
  AND NOT COALESCE(item->>'kind' IN ('tool','bundle') AND restriction->>'concurrency' IN ('sequential','shared_read') AND jsonb_typeof(restriction->'concurrency') = 'string',false) THEN RETURN false; END IF;
  IF jsonb_typeof(COALESCE(restriction->'scope','{}'::jsonb)) <> 'object' OR jsonb_typeof(COALESCE(restriction->'limits','{}'::jsonb)) <> 'object' THEN RETURN false; END IF;
  FOR candidate IN SELECT v FROM jsonb_each(COALESCE(restriction->'scope','{}'::jsonb)) AS fields(k,v) LOOP
   IF jsonb_typeof(candidate) <> 'array' OR jsonb_array_length(candidate) = 0 OR jsonb_path_exists(candidate,'strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
  END LOOP;
  FOR candidate IN SELECT v FROM jsonb_each(COALESCE(restriction->'limits','{}'::jsonb)) AS fields(k,v) LOOP
   IF jsonb_typeof(candidate) <> 'number' OR NOT candidate::text ~ '^[1-9][0-9]*$' OR candidate::text::numeric > 18446744073709551615 THEN RETURN false; END IF;
  END LOOP;
  IF COALESCE(item->'alias','null'::jsonb) <> 'null'::jsonb
  AND (jsonb_typeof(item->'alias') <> 'string' OR item->>'kind' <> 'tool' OR NOT COALESCE(item->>'alias' ~ '^[A-Za-z0-9_-]{1,64}$',false)) THEN RETURN false; END IF;
  IF COALESCE(item->'members','null'::jsonb) <> 'null'::jsonb THEN
   IF item->>'kind' <> 'bundle' OR jsonb_typeof(item->'members') <> 'array'
   OR jsonb_array_length(item->'members') > 128
   OR jsonb_path_exists(item->'members','strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
   IF (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements(item->'members') e) THEN RETURN false; END IF;
  END IF;
 END LOOP;
 IF (SELECT count(*) <> count(DISTINCT e->'target') FROM jsonb_array_elements(edges) e) THEN RETURN false; END IF;
 FOR item IN SELECT jsonb_array_elements(removals) LOOP
  IF jsonb_typeof(item) <> 'string' OR NOT (item #>> '{}') = ANY(removable) THEN RETURN false; END IF;
 END LOOP;
 IF (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements(removals) e) THEN RETURN false; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(edges) e WHERE e->>'kind' = 'skill')
 AND removals ?| support THEN RETURN false; END IF;
 RETURN length(btrim(COALESCE(value->>'instructions',''), U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0
 OR EXISTS(SELECT 1 FROM jsonb_array_elements(edges) e WHERE e->>'kind' IN ('skill','source'))
 OR EXISTS(SELECT 1 FROM jsonb_array_elements(plugins) p WHERE jsonb_array_length(p->'skills') > 0);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;
