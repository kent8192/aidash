SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE FUNCTION public.aidash_agent_before_provider_tools_0050(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE plugins jsonb; plugin jsonb; plugin_settings jsonb; plugin_choices jsonb; server_choice jsonb; declared jsonb; completion jsonb; check_item jsonb; item jsonb; removals jsonb; edges jsonb; step_count numeric; parallelism numeric; context_policy jsonb; summary jsonb; restriction jsonb; candidate jsonb; policy jsonb; deferred boolean; budget text; bound numeric; removable text[]; support text[];
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR value - ARRAY['schema_version','model','instructions','bindings','remove_default','max_steps','projection_version','harness_contract','prompt_cache','exposure','tool_parallelism','context_policy','completion','agent_plugins','agent_plugin_settings','reference_embedding']::text[] <> '{}'::jsonb
 OR NOT COALESCE(value->'projection_version','null'::jsonb) IN ('null'::jsonb,'"legacy"'::jsonb,'"ordered"'::jsonb,'"native"'::jsonb)
 OR NOT COALESCE(value->'harness_contract','null'::jsonb) IN ('null'::jsonb,'"legacy@1"'::jsonb,'"structured@3"'::jsonb)
 OR NOT COALESCE(value->'prompt_cache','null'::jsonb) IN ('null'::jsonb,'"off"'::jsonb,'"explicit"'::jsonb)
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb
 OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_deployment":"aidash://contract"}'::jsonb),false)
 OR (COALESCE(value->'reference_embedding','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'reference_embedding' || '{"registry_deployment":"aidash://contract"}'::jsonb),false))
 OR jsonb_typeof(COALESCE(value->'instructions','""'::jsonb)) <> 'string' THEN RETURN false; END IF;
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
  removable := ARRAY['workspace_observe','workspace_search','workspace_wait','skill_asset_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','attachment_copy','memory_mutate','memory_recall','memory_reflect','compaction'];
  support := ARRAY['skill_asset_read'];
 ELSE
  removable := ARRAY['workspace_observe','workspace_search','workspace_wait','skill_list','skill_load','skill_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','attachment_copy','memory_mutate','memory_recall','memory_reflect','compaction'];
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
$_$;

--

CREATE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $$
DECLARE item jsonb; restriction jsonb; edges jsonb := '[]'::jsonb;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR jsonb_typeof(COALESCE(value->'bindings','[]'::jsonb)) IS DISTINCT FROM 'array' THEN RETURN false; END IF;
 FOR item IN SELECT jsonb_array_elements(COALESCE(value->'bindings','[]'::jsonb)) LOOP
  restriction := COALESCE(item->'narrow','{}'::jsonb);
  IF jsonb_typeof(item) IS DISTINCT FROM 'object' OR jsonb_typeof(restriction) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
  IF COALESCE(restriction->'provider_tool','null'::jsonb) <> 'null'::jsonb
  AND NOT COALESCE(item->>'kind' IN ('tool','bundle') AND public.aidash_provider_tool_binding_is_valid(restriction->'provider_tool'),false) THEN RETURN false; END IF;
  item := jsonb_set(item, '{narrow}', restriction - 'provider_tool');
  edges := edges || jsonb_build_array(item);
 END LOOP;
 RETURN public.aidash_agent_before_provider_tools_0050(jsonb_set(value, '{bindings}', edges));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

--

CREATE FUNCTION public.aidash_binding_package_is_valid(value jsonb, entry_id text, entry_version text) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE entity jsonb; candidate jsonb;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['entity','author','permissions','dependencies']::text[] <> '{}'::jsonb
 OR jsonb_typeof(value->'author') <> 'string' OR length(btrim(value->>'author', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) = 0
 OR jsonb_typeof(value->'permissions') <> 'array' OR jsonb_typeof(value->'dependencies') <> 'array'
 OR jsonb_path_exists(value->'permissions','strict $[*] ? (@.type() != "string")') THEN RETURN false; END IF;
 entity := value->'entity';
 IF NOT COALESCE(public.aidash_package_entry_shape_is_valid(entity),false)
 OR entity ?| ARRAY['installation','binding_normalization']
 OR jsonb_typeof(entity->'id') IS DISTINCT FROM 'string' OR jsonb_typeof(entity->'version') IS DISTINCT FROM 'string'
 OR entity->>'id' IS DISTINCT FROM entry_id OR entity->>'version' IS DISTINCT FROM entry_version
 OR NOT COALESCE(public.aidash_qualified_ref_is_valid(jsonb_build_object('registry_deployment','aidash://contract','id',entry_id,'version',entry_version)),false) THEN RETURN false; END IF;
 FOR candidate IN SELECT jsonb_array_elements(value->'dependencies') LOOP
  IF NOT COALESCE(public.aidash_qualified_ref_is_valid(candidate || '{"registry_deployment":"aidash://contract"}'::jsonb),false) THEN RETURN false; END IF;
 END LOOP;
 RETURN COALESCE(CASE entity->>'kind'
 WHEN 'agent' THEN public.aidash_agent_bindings_is_valid(entity->'config')
 WHEN 'tool' THEN public.aidash_descriptor_is_valid(entity->'config') AND entity #>> '{config,tier}' <> 'builtin'
 WHEN 'bundle' THEN public.aidash_bundle_is_valid(entity->'config')
 WHEN 'memory' THEN CASE WHEN entity->'config' ? 'schema_version' THEN public.aidash_context_is_valid(entity->'config','memory') ELSE public.aidash_native_context_is_valid(entity->'config','memory') END
 WHEN 'source' THEN CASE WHEN entity->'config' ? 'schema_version' THEN public.aidash_context_is_valid(entity->'config','source') ELSE public.aidash_native_context_is_valid(entity->'config','source') END
 WHEN 'skill' THEN jsonb_typeof(entity #> '{config,instructions}') = 'string' AND length(btrim(entity #>> '{config,instructions}', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0
 ELSE false END,false);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$_$;

--

CREATE FUNCTION public.aidash_bundle_is_valid(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $$
DECLARE candidate jsonb;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - 'members' <> '{}'::jsonb
 OR jsonb_typeof(value->'members') <> 'array' THEN RETURN false; END IF;
 IF jsonb_array_length(value->'members') NOT BETWEEN 1 AND 128 THEN RETURN false; END IF;
 FOR candidate IN SELECT jsonb_array_elements(value->'members') LOOP
  IF NOT public.aidash_qualified_ref_is_valid(candidate) THEN RETURN false; END IF;
 END LOOP;
 RETURN (SELECT count(*) = count(DISTINCT elements.item->>'id')
 FROM jsonb_array_elements(value->'members') AS elements(item));
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;

--

CREATE FUNCTION public.aidash_context_is_valid(value jsonb, kind text) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE source jsonb; adapter text;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['schema_version','source','context_source']::text[] <> '{}'::jsonb
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb OR jsonb_typeof(value->'source') <> 'object' THEN RETURN false; END IF;
 source := value->'source'; adapter := source->>'adapter';
 IF COALESCE(value->'context_source','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_source_declaration_is_valid(value->'context_source', CASE adapter WHEN 'private_references' THEN 'references' WHEN 'reference_attachments' THEN 'references' WHEN 'skill_attachments' THEN 'skills' WHEN 'skill_roots' THEN 'skills' ELSE adapter END),false) THEN RETURN false; END IF;
 IF kind = 'memory' THEN RETURN adapter IN ('conversation_memory','semantic_memory') AND source - 'adapter' = '{}'::jsonb; END IF;
 IF kind <> 'source' THEN RETURN false; END IF;
 RETURN CASE adapter
 WHEN 'workspace_observation' THEN source - 'adapter' = '{}'::jsonb
 WHEN 'workspace_retrieval' THEN source - ARRAY['adapter','on_unavailable']::text[] = '{}'::jsonb
  AND (NOT source ? 'on_unavailable' OR (jsonb_typeof(source->'on_unavailable') = 'string' AND source->>'on_unavailable' IN ('pause','degrade')))
 WHEN 'private_references' THEN source - ARRAY['adapter','digest']::text[] = '{}'::jsonb AND source->>'digest' ~ '^[a-fA-F0-9]{64}$'
 WHEN 'reference_attachments' THEN source - ARRAY['adapter','references']::text[] = '{}'::jsonb AND jsonb_typeof(source->'references') = 'array' AND jsonb_array_length(source->'references') BETWEEN 1 AND 256
 WHEN 'skill_attachments' THEN source - ARRAY['adapter','attachments']::text[] = '{}'::jsonb AND jsonb_typeof(source->'attachments') = 'array' AND jsonb_array_length(source->'attachments') BETWEEN 1 AND 16
 WHEN 'skill_roots' THEN source - ARRAY['adapter','roots']::text[] = '{}'::jsonb AND jsonb_typeof(source->'roots') = 'array' AND jsonb_array_length(source->'roots') BETWEEN 1 AND 8 AND NOT jsonb_path_exists(source->'roots','strict $[*] ? (@.type() != "string")')
 ELSE false END;
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$_$;

--

CREATE FUNCTION public.aidash_descriptor_before_provider_tools_0050(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE expected_provider text; expected_tier text; start_operation boolean;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['registry_deployment','provider','operation','default_alias','tier','narrow','transport','lifecycle']::text[] <> '{}'::jsonb
 OR NOT COALESCE(value->>'registry_deployment' ~ '^aidash://[A-Za-z0-9-]{1,100}$', false)
 OR NOT COALESCE(value->>'default_alias' ~ '^[A-Za-z0-9_-]{1,64}$', false)
 OR jsonb_typeof(COALESCE(value->'narrow','{}'::jsonb)) <> 'object' THEN RETURN false; END IF;
 IF COALESCE(value->'narrow','{}'::jsonb) - ARRAY['allowed_hosts','scope','limits','concurrency']::text[] <> '{}'::jsonb THEN RETURN false; END IF;
 -- shared_read may only restate a provider's own declaration: the core reads below.
 IF COALESCE(value #> '{narrow,concurrency}','null'::jsonb) <> 'null'::jsonb
 AND NOT COALESCE(jsonb_typeof(value #> '{narrow,concurrency}') = 'string'
  AND (value #>> '{narrow,concurrency}' = 'sequential'
   OR (value #>> '{narrow,concurrency}' = 'shared_read' AND value->>'operation' IN ('file_read','file_search','skill_list','skill_load'))),false) THEN RETURN false; END IF;
 expected_provider := CASE value->>'operation'
  WHEN 'capability_search' THEN 'core.exposure@1' WHEN 'capability_describe' THEN 'core.exposure@1'
  WHEN 'capability_load' THEN 'core.exposure@1' WHEN 'capability_unload' THEN 'core.exposure@1'
  WHEN 'skill_asset_read' THEN 'core.skills@1'
  WHEN 'workspace_read' THEN 'core.workspace@1' WHEN 'workspace_observe' THEN 'core.workspace@1'
  WHEN 'workspace_search' THEN 'core.workspace@1' WHEN 'workspace_wait' THEN 'core.workspace@1' WHEN 'workspace_message' THEN 'core.workspace@1'
  WHEN 'human_request' THEN 'core.human@1' WHEN 'connection_request' THEN 'core.human@1'
  WHEN 'subnode_create' THEN 'core.subnodes@1' WHEN 'subnode_task' THEN 'core.subnodes@1'
  WHEN 'subnode_add_member' THEN 'core.subnodes@1' WHEN 'subnode_remove_member' THEN 'core.subnodes@1'
  WHEN 'subnode_dissolve' THEN 'core.subnodes@1' WHEN 'subnode_message' THEN 'core.subnodes@1'
  WHEN 'task_create' THEN 'core.tasks@1' WHEN 'task_delegate' THEN 'core.tasks@1'
  WHEN 'agent_discover' THEN 'core.tasks@1' WHEN 'task_assign' THEN 'core.tasks@1'
  WHEN 'skill_list' THEN 'core.skills@1' WHEN 'skill_load' THEN 'core.skills@1' WHEN 'skill_read' THEN 'core.skills@1'
  WHEN 'file_search' THEN 'core.files@1' WHEN 'file_read' THEN 'core.files@1' WHEN 'apply_patch' THEN 'core.files@1' WHEN 'attachment_copy' THEN 'core.files@1'
  WHEN 'artifact_publish' THEN 'core.artifacts@1' WHEN 'memory_mutate' THEN 'core.memory@1' WHEN 'memory_recall' THEN 'core.memory@1' WHEN 'memory_reflect' THEN 'core.memory@1'
  WHEN 'shell' THEN 'core.sandbox@1' WHEN 'shell_poll' THEN 'core.sandbox@1' WHEN 'shell_cancel' THEN 'core.sandbox@1'
  WHEN 'code_interpreter' THEN 'core.sandbox@1' WHEN 'python_install' THEN 'core.sandbox@1'
  WHEN 'python_poll' THEN 'core.sandbox@1' WHEN 'python_cancel' THEN 'core.sandbox@1'
  WHEN 'outbound_get' THEN 'core.egress@1' WHEN 'file_share' THEN 'core.sharing@1'
  WHEN 'live_preview_start' THEN 'core.preview@1' WHEN 'live_preview_stop' THEN 'core.preview@1' END;
 IF expected_provider IS NOT NULL THEN
  expected_tier := CASE WHEN value->>'operation' IN ('shell','shell_poll','shell_cancel','code_interpreter','python_install','python_poll','python_cancel','outbound_get','apply_patch','file_share','live_preview_start','live_preview_stop') THEN 'sandboxed' ELSE 'builtin' END;
  IF value->>'provider' IS DISTINCT FROM expected_provider OR value->>'tier' IS DISTINCT FROM expected_tier
  OR COALESCE(value->'transport','null'::jsonb) <> 'null'::jsonb THEN RETURN false; END IF;
  start_operation := value->>'operation' IN ('shell','code_interpreter','python_install');
  IF start_operation THEN
   IF jsonb_typeof(value->'lifecycle') IS DISTINCT FROM 'object'
   OR (value->'lifecycle') - ARRAY['poll','cancel']::text[] <> '{}'::jsonb
   OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value #> '{lifecycle,poll}'),false)
   OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value #> '{lifecycle,cancel}'),false)
   OR value #> '{lifecycle,poll}' = value #> '{lifecycle,cancel}'
   OR value #>> '{lifecycle,poll,registry_deployment}' IS DISTINCT FROM value->>'registry_deployment'
   OR value #>> '{lifecycle,cancel,registry_deployment}' IS DISTINCT FROM value->>'registry_deployment' THEN RETURN false; END IF;
  ELSIF COALESCE(value->'lifecycle','null'::jsonb) <> 'null'::jsonb THEN RETURN false; END IF;
  RETURN true;
 END IF;
 -- Research contract Node providers: each implements fixed operations at a
 -- fixed tier, with no transport or lifecycle.
 IF value->>'provider' LIKE 'research.%' THEN
  RETURN COALESCE((value->>'provider', value->>'operation', value->>'tier') IN (
    ('research.brave@1', 'search', 'research'),
    ('research.page@1', 'fetch', 'research'), ('research.page@1', 'find', 'research'),
    ('research.page@1', 'view', 'research'),
    ('research.fixture@1', 'search', 'fixture'), ('research.fixture@1', 'fetch', 'fixture'),
    ('research.fixture@1', 'find', 'fixture'), ('research.fixture@1', 'view', 'fixture'))
  AND COALESCE(value->'transport','null'::jsonb) = 'null'::jsonb
  AND COALESCE(value->'lifecycle','null'::jsonb) = 'null'::jsonb, false);
 END IF;
 RETURN COALESCE(value->>'tier' = 'integration' AND value->>'operation' = 'invoke'
 AND value->>'provider' = 'integration.' || (value #>> '{transport,transport}') || '@1'
 AND value #>> '{transport,transport}' IN ('http','mcp','agent')
 AND COALESCE(value->'lifecycle','null'::jsonb) = 'null'::jsonb
 AND public.aidash_tool_config_is_valid(value->'transport'),false);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$_$;

--

CREATE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $$
DECLARE operation text;
BEGIN
 operation := CASE value->>'provider'
 WHEN 'provider.openrouter.advisor@1' THEN 'provider_advisor' WHEN 'openrouter:advisor' THEN 'provider_advisor'
 WHEN 'provider.openrouter.subagent@1' THEN 'provider_subagent' WHEN 'openrouter:subagent' THEN 'provider_subagent'
 WHEN 'provider.openrouter.experimental__search_models@1' THEN 'provider_search_models' WHEN 'openrouter:experimental__search_models' THEN 'provider_search_models'
 WHEN 'provider.openrouter.tool_search@1' THEN 'provider_tool_search' WHEN 'openrouter:tool_search' THEN 'provider_tool_search'
 WHEN 'provider.openrouter.shell@1' THEN 'provider_shell' WHEN 'openrouter:shell' THEN 'provider_shell'
 END;
 IF operation IS NULL THEN RETURN public.aidash_descriptor_is_valid_before_warning_0051(value); END IF;
 IF value->>'operation' IS DISTINCT FROM operation OR value->>'default_alias' IS DISTINCT FROM operation THEN RETURN false; END IF;
 -- Reuse the established descriptor shape/tier/no-transport rules, without relaxing them.
 RETURN public.aidash_descriptor_is_valid_before_warning_0051(value || '{"provider":"provider.openrouter.datetime@1","operation":"provider_datetime","default_alias":"provider_datetime"}'::jsonb);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

--

CREATE FUNCTION public.aidash_descriptor_is_valid_before_warning_0051(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE restriction jsonb; operation text;
BEGIN
 restriction := COALESCE(value->'narrow','{}'::jsonb);
 IF jsonb_typeof(restriction) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 -- Definition parameters are not an author-controlled Binding. Only the
 -- Deployment's canonical, immutable descriptor can claim the Provider tier.
 IF COALESCE(restriction->'provider_tool','null'::jsonb) <> 'null'::jsonb THEN RETURN false; END IF;
 restriction := restriction - 'provider_tool';
 IF value->>'tier' = 'provider' OR value->>'provider' LIKE 'provider.openrouter.%' OR value->>'provider' LIKE 'openrouter:%' THEN
  operation := CASE value->>'provider'
   WHEN 'provider.openrouter.web_search@1' THEN 'provider_web_search' WHEN 'openrouter:web_search' THEN 'provider_web_search'
   WHEN 'provider.openrouter.web_fetch@1' THEN 'provider_web_fetch' WHEN 'openrouter:web_fetch' THEN 'provider_web_fetch'
   WHEN 'provider.openrouter.datetime@1' THEN 'provider_datetime' WHEN 'openrouter:datetime' THEN 'provider_datetime'
   WHEN 'provider.openrouter.fusion@1' THEN 'provider_fusion' WHEN 'openrouter:fusion' THEN 'provider_fusion' END;
  RETURN COALESCE(jsonb_typeof(value) = 'object'
   AND value - ARRAY['registry_deployment','provider','operation','default_alias','tier','narrow','transport','lifecycle']::text[] = '{}'::jsonb
   AND value->>'registry_deployment' ~ '^aidash://[A-Za-z0-9-]{1,100}$'
   AND operation IS NOT NULL AND value->>'operation' = operation AND value->>'default_alias' = operation
   AND value->>'tier' = 'provider' AND restriction = '{}'::jsonb
   AND COALESCE(value->'transport','null'::jsonb) = 'null'::jsonb
   AND COALESCE(value->'lifecycle','null'::jsonb) = 'null'::jsonb,false);
 END IF;
 RETURN public.aidash_descriptor_before_provider_tools_0050(jsonb_set(value, '{narrow}', restriction));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $_$;

--

CREATE FUNCTION public.aidash_estimate_diagnostics_valid(config jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE projection text; calibration jsonb;
BEGIN
  IF jsonb_typeof(config) <> 'object'
     OR config - ARRAY['token_encoding','calibrations']::text[] <> '{}'::jsonb
     OR (config ? 'token_encoding' AND config->'token_encoding' NOT IN
         ('null'::jsonb, '"o200k_base"'::jsonb, '"gemma4"'::jsonb))
  THEN RETURN false; END IF;
  IF NOT config ? 'calibrations' THEN RETURN true; END IF;
  IF jsonb_typeof(config->'calibrations') <> 'object' THEN RETURN false; END IF;
  FOR projection, calibration IN SELECT key, value FROM jsonb_each(config->'calibrations') LOOP
    IF projection NOT IN ('legacy','ordered','native') OR jsonb_typeof(calibration) <> 'object'
       OR NOT calibration ?& ARRAY['ratio_ppm','samples']::text[]
       OR calibration - ARRAY['ratio_ppm','samples']::text[] <> '{}'::jsonb
       OR jsonb_typeof(calibration->'ratio_ppm') <> 'number'
       OR jsonb_typeof(calibration->'samples') <> 'number'
       OR (calibration->'ratio_ppm')::text !~ '^[0-9]+$'
       OR (calibration->'samples')::text !~ '^[0-9]+$'
    THEN RETURN false; END IF;
    IF (calibration->>'ratio_ppm')::numeric NOT BETWEEN 1 AND 1000000
       OR (calibration->>'samples')::numeric NOT BETWEEN 0 AND 200
    THEN RETURN false; END IF;
  END LOOP;
  RETURN true;
END $_$;

--

CREATE FUNCTION public.aidash_media_routes_valid(routes jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE
    route jsonb;
    media_format jsonb;
    verified_at timestamptz;
    expires_at timestamptz;
BEGIN
    IF jsonb_typeof(routes) <> 'array' THEN RETURN false; END IF;
    FOR route IN SELECT value FROM jsonb_array_elements(routes) AS entries(value) LOOP
        IF jsonb_typeof(route) <> 'object' THEN RETURN false; END IF;
        IF NOT route ?& ARRAY['tag','formats','source','verified_at','expires_at']::text[]
           OR (route - ARRAY['tag','formats','source','verified_at','expires_at']::text[]) <> '{}'::jsonb
           OR jsonb_typeof(route->'tag') <> 'string'
           OR jsonb_typeof(route->'formats') <> 'array'
           OR jsonb_typeof(route->'source') <> 'string'
           OR jsonb_typeof(route->'verified_at') <> 'string'
           OR jsonb_typeof(route->'expires_at') <> 'string'
        THEN RETURN false; END IF;
        IF route->>'tag' !~ '^[A-Za-z0-9._/-]+$'
           OR length(btrim(route->>'source')) = 0
           OR jsonb_array_length(route->'formats') = 0
        THEN RETURN false; END IF;
        FOR media_format IN SELECT value FROM jsonb_array_elements(route->'formats') AS formats(value) LOOP
            IF jsonb_typeof(media_format) <> 'string'
               OR media_format #>> '{}' NOT IN ('image/png','image/jpeg','image/gif','image/webp','wav','mp3','m4a','aac','ogg','webm','flac')
            THEN RETURN false; END IF;
        END LOOP;
        IF route->>'verified_at' !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]{1,9})?(Z|[+-][0-9]{2}:[0-9]{2})$'
           OR route->>'expires_at' !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]{1,9})?(Z|[+-][0-9]{2}:[0-9]{2})$'
        THEN RETURN false; END IF;
        BEGIN
            verified_at := (route->>'verified_at')::timestamptz;
            expires_at := (route->>'expires_at')::timestamptz;
        EXCEPTION WHEN OTHERS THEN
            RETURN false;
        END;
        IF NOT isfinite(verified_at) OR NOT isfinite(expires_at) OR verified_at >= expires_at
        THEN RETURN false; END IF;
    END LOOP;
    RETURN true;
END $_$;

--

CREATE FUNCTION public.aidash_native_context_before_sources(value jsonb, kind text) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE policy jsonb; bounds jsonb;
BEGIN
 IF kind = 'memory' THEN
  IF NOT public.jsonb_matches_schema($native_memory${"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/$defs/ProviderConfig","$defs":{"ProviderConfig":{"type":"object","properties":{"engine":{"type":"string","enum":["hindsight_rust"]},"policy":{"$ref":"#/$defs/Policy"}},"required":["engine","policy"],"additionalProperties":false},"SourceConfig":{"type":"object","properties":{"scope":{"type":"string","enum":["participant","workspace"]},"memory":{"$ref":"#/$defs/EntityRef"},"max_tokens":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["scope","memory","max_tokens"],"additionalProperties":false},"Policy":{"type":"object","properties":{"extraction":{"$ref":"#/$defs/EntityRef"},"derivation":{"$ref":"#/$defs/EntityRef"},"reflection":{"$ref":"#/$defs/EntityRef"},"embedding":{"$ref":"#/$defs/EntityRef"},"reranker":{"$ref":"#/$defs/EntityRef"},"tokenizer":{"$ref":"#/$defs/EntityRef"},"semantic_link_min_similarity_millionths":{"type":"integer","minimum":1,"maximum":1000000},"prices":{"$ref":"#/$defs/Prices"},"retention":{"$ref":"#/$defs/Retention"},"bounds":{"$ref":"#/$defs/Bounds"},"learn_from_runs":{"type":"boolean"},"maintain_observations":{"type":"boolean"},"refresh_mental_models":{"type":"boolean"}},"required":["extraction","derivation","reflection","embedding","reranker","tokenizer","semantic_link_min_similarity_millionths","prices","retention","bounds","learn_from_runs","maintain_observations","refresh_mental_models"],"additionalProperties":false},"Retention":{"type":"object","properties":{"unit_max_age_days":{"type":["integer","null"],"minimum":1,"maximum":3650},"candidate_days":{"type":"integer","minimum":1,"maximum":3650},"history_days":{"type":"integer","minimum":1,"maximum":3650},"history_versions":{"type":"integer","minimum":1,"maximum":1024},"model_result_days":{"type":"integer","minimum":1,"maximum":3650},"backup_days":{"type":"integer","minimum":1,"maximum":3650},"purge_after_seconds":{"type":"integer","minimum":1,"maximum":86400},"purge_batch":{"type":"integer","minimum":1,"maximum":1024},"max_unit_records":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_operations":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["candidate_days","history_days","history_versions","model_result_days","backup_days","purge_after_seconds","purge_batch","max_unit_records","max_model_operations"],"additionalProperties":false},"Prices":{"type":"object","properties":{"extraction":{"$ref":"#/$defs/Rate"},"derivation":{"$ref":"#/$defs/Rate"},"reflection":{"$ref":"#/$defs/Rate"},"embedding":{"$ref":"#/$defs/Rate"},"reranker":{"$ref":"#/$defs/Rate"}},"required":["extraction","derivation","reflection","embedding","reranker"],"additionalProperties":false},"Rate":{"type":"object","properties":{"input_per_million":{"type":"integer","minimum":0,"maximum":18446744073709551615},"output_per_million":{"type":"integer","minimum":0,"maximum":18446744073709551615}},"required":["input_per_million","output_per_million"],"additionalProperties":false},"Bounds":{"type":"object","properties":{"max_unit_bytes":{"type":"integer","minimum":1,"maximum":2147483647},"max_input_bytes":{"type":"integer","minimum":1,"maximum":2147483647},"max_units":{"type":"integer","minimum":1,"maximum":2147483647},"max_candidates":{"type":"integer","minimum":1,"maximum":2147483647},"max_entities":{"type":"integer","minimum":1,"maximum":2147483647},"max_evidence":{"type":"integer","minimum":1,"maximum":2147483647},"max_links":{"type":"integer","minimum":1,"maximum":2147483647},"max_graph_hops":{"type":"integer","minimum":1,"maximum":2147483647},"max_graph_visits":{"type":"integer","minimum":1,"maximum":2147483647},"max_results":{"type":"integer","minimum":1,"maximum":2147483647},"max_context_tokens":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_calls":{"type":"integer","minimum":1,"maximum":2147483647},"max_model_tokens":{"type":"integer","minimum":1,"maximum":2147483647},"max_cost_micros":{"type":"integer","minimum":1,"maximum":9223372036854775807},"max_retries":{"type":"integer","minimum":1,"maximum":2147483647},"max_call_seconds":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["max_unit_bytes","max_input_bytes","max_units","max_candidates","max_entities","max_evidence","max_links","max_graph_hops","max_graph_visits","max_results","max_context_tokens","max_model_calls","max_model_tokens","max_cost_micros","max_retries","max_call_seconds"],"additionalProperties":false},"EntityRef":{"type":"object","required":["id","version"],"properties":{"id":{"type":"string","minLength":1},"version":{"type":"string","pattern":"^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:[-+].+)?$"}},"additionalProperties":false}}}$native_memory$::json,value) THEN RETURN false; END IF;
  policy := value->'policy'; bounds := policy->'bounds';
  RETURN (bounds->>'max_candidates')::numeric <= (bounds->>'max_units')::numeric
    AND (bounds->>'max_results')::numeric <= (bounds->>'max_candidates')::numeric
    AND (policy #>> '{retention,max_unit_records}')::numeric >= (bounds->>'max_units')::numeric;
 ELSIF kind = 'source' THEN
  IF NOT public.jsonb_matches_schema($native_source${"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"#/$defs/SourceConfig","$defs":{"SourceConfig":{"type":"object","properties":{"scope":{"type":"string","enum":["participant","workspace"]},"memory":{"$ref":"#/$defs/EntityRef"},"max_tokens":{"type":"integer","minimum":1,"maximum":2147483647}},"required":["scope","memory","max_tokens"],"additionalProperties":false},"EntityRef":{"type":"object","required":["id","version"],"properties":{"id":{"type":"string","minLength":1},"version":{"type":"string","pattern":"^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:[-+].+)?$"}},"additionalProperties":false}}}$native_source$::json,value) THEN RETURN false; END IF;
  RETURN true;
 ELSE RETURN false; END IF;
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$_$;

--

CREATE FUNCTION public.aidash_native_context_is_valid(value jsonb, kind text) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $$
BEGIN
 IF COALESCE(value->'context_source','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_source_declaration_is_valid(value->'context_source','semantic_memory'),false) THEN RETURN false; END IF;
 RETURN public.aidash_native_context_before_sources(value - 'context_source',kind);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

--

CREATE FUNCTION public.aidash_package_entry_shape_is_valid(metadata jsonb) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $_$ SELECT COALESCE(((jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'name'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."name".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'description'::text)) = 'object'::text) AND (((metadata - 'installation'::text) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - 'installation'::text), 'strict $."description".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - 'installation'::text) -> 'config'::text)) = 'object'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb)) = 'object'::text) AND public.jsonschema_is_valid((COALESCE(((metadata - 'installation'::text) -> 'schema'::text), '{}'::jsonb))::json) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND
						CASE
						WHEN (jsonb_typeof((metadata - 'installation'::text)) = 'object'::text) THEN (((metadata - 'installation'::text) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - 'installation'::text) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((NOT (metadata ? 'installation'::text)) OR ((jsonb_typeof((metadata -> 'installation'::text)) = 'object'::text) AND (((metadata -> 'installation'::text) ->> 'contract'::text) = '1'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'tenant'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'installation'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'revision'::text)) = 'number'::text)))),false) $_$;

--

CREATE FUNCTION public.aidash_package_source_matches(input_manifest jsonb, input_source text) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE
    AS $$
BEGIN
    IF input_source IS NULL THEN
        RETURN false;
    END IF;
    RETURN input_source::jsonb = input_manifest;
EXCEPTION WHEN OTHERS THEN
    RETURN false;
END
$$;

--

CREATE FUNCTION public.aidash_provider_route_valid(route jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
BEGIN
  IF jsonb_typeof(route) <> 'object'
     OR NOT route ?& ARRAY['tag','source','verified_at','expires_at']::text[]
     OR route - ARRAY['tag','source','verified_at','expires_at']::text[] <> '{}'::jsonb
     OR jsonb_typeof(route->'tag') <> 'string'
     OR length(route->>'tag') NOT BETWEEN 1 AND 256
     OR route->>'tag' !~ '^[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)*$'
     OR route->>'tag' ~ '(^|/)[.]{1,2}(/|$)'
     OR jsonb_typeof(route->'source') <> 'string'
     OR length(route->>'source') NOT BETWEEN 1 AND 1024
     OR route->>'source' ~ '[[:cntrl:]]'
  THEN RETURN false; END IF;
  -- Reuse the established exact timestamp and provenance validation.
  RETURN COALESCE(public.aidash_media_routes_valid(
      jsonb_build_array(route || '{"formats":["image/png"]}'::jsonb)), false);
END $_$;

--

CREATE FUNCTION public.aidash_provider_tool_binding_is_valid(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE parameters jsonb; key text;
BEGIN
 IF public.aidash_provider_tool_binding_is_valid_before_warning_0051(value) THEN RETURN true; END IF;
 IF NOT public.jsonb_matches_schema($warning_tools${"type":"object","additionalProperties":false,"required":["max_tool_calls","parameters"],"properties":{"max_tool_calls":{"type":"integer","minimum":1,"maximum":8},"parameters":{"oneOf":[{"type":"object","additionalProperties":false,"required":["model","max_completion_tokens","forward_transcript","stream"],"properties":{"model":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}},"max_completion_tokens":{"type":"integer","minimum":1,"maximum":8192},"forward_transcript":{"type":"boolean"},"stream":{"const":false},"instructions":{"type":"string","maxLength":8192}}},{"type":"object","additionalProperties":false,"required":["model","max_completion_tokens","max_tool_calls","tools","inherit_functions"],"properties":{"model":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}},"max_completion_tokens":{"type":"integer","minimum":1,"maximum":8192},"max_tool_calls":{"type":"integer","minimum":1,"maximum":4},"tools":{"const":[]},"instructions":{"type":"string","maxLength":8192},"inherit_functions":{"const":false}}},{"type":"object","additionalProperties":false,"required":["max_results"],"properties":{"max_results":{"type":"integer","minimum":1,"maximum":10}}},{"type":"object","additionalProperties":false,"required":["engine","environment"],"properties":{"engine":{"const":"openrouter"},"environment":{"type":"object","additionalProperties":false,"required":["type","network_policy"],"properties":{"type":{"const":"container_reference"},"network_policy":{"oneOf":[{"type":"object","additionalProperties":false,"required":["type"],"properties":{"type":{"const":"disabled"}}},{"type":"object","additionalProperties":false,"required":["type","allowed_domains"],"properties":{"type":{"const":"allowlist"},"allowed_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$"}}}}]}}}}}]}},"$schema":"https://json-schema.org/draft/2020-12/schema"}$warning_tools$::json, value) THEN RETURN false; END IF;
 IF NOT (value->>'max_tool_calls') ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 parameters := value->'parameters';
 FOREACH key IN ARRAY ARRAY['max_results','max_completion_tokens','max_tool_calls'] LOOP
  IF parameters ? key AND NOT (parameters->>key) ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 END LOOP;
 RETURN true;
EXCEPTION WHEN OTHERS THEN RETURN false;
END $_$;

--

CREATE FUNCTION public.aidash_provider_tool_binding_is_valid_before_warning_0051(value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
DECLARE parameters jsonb; key text;
BEGIN
 IF NOT public.jsonb_matches_schema($provider_tools${"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"required":["max_tool_calls","parameters"],"properties":{"max_tool_calls":{"type":"integer","minimum":1,"maximum":8},"parameters":{"oneOf":[{"type":"object","additionalProperties":false,"required":["engine","max_results","max_total_results"],"properties":{"engine":{"enum":["exa","parallel","perplexity"]},"max_results":{"type":"integer","minimum":1,"maximum":10},"max_total_results":{"type":"integer","minimum":1,"maximum":30},"allowed_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}},"excluded_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}}},"not":{"required":["allowed_domains","excluded_domains"]}},{"type":"object","additionalProperties":false,"required":["engine","max_content_tokens"],"properties":{"engine":{"enum":["openrouter","exa","parallel"]},"max_content_tokens":{"type":"integer","minimum":1,"maximum":16384},"allowed_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}},"blocked_domains":{"type":"array","minItems":1,"maxItems":32,"items":{"type":"string","minLength":1,"maxLength":253,"pattern":"^[a-zA-Z0-9.-]+$"}}}},{"type":"object","additionalProperties":false,"required":["timezone"],"properties":{"timezone":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[-A-Za-z0-9/_+]+$"}}},{"type":"object","additionalProperties":false,"required":["analysis_models","model","max_tool_calls","max_completion_tokens"],"properties":{"analysis_models":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}}},"model":{"type":"string","minLength":3,"maxLength":200,"pattern":"^[a-zA-Z0-9_~.-]+/[a-zA-Z0-9_~./-]+$","not":{"pattern":"^openrouter/"}},"max_tool_calls":{"type":"integer","minimum":1,"maximum":4},"max_completion_tokens":{"type":"integer","minimum":1,"maximum":8192}}}]}}}$provider_tools$::json, value) THEN RETURN false; END IF;
 parameters := value->'parameters';
 IF NOT (value->>'max_tool_calls') ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 FOREACH key IN ARRAY ARRAY['max_results','max_total_results','max_content_tokens','max_tool_calls','max_completion_tokens'] LOOP
  IF parameters ? key AND NOT (parameters->>key) ~ '^[1-9][0-9]*$' THEN RETURN false; END IF;
 END LOOP;
 IF parameters ? 'max_results' AND (parameters->>'max_total_results')::numeric < (parameters->>'max_results')::numeric THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN OTHERS THEN RETURN false;
END $_$;

--

CREATE FUNCTION public.aidash_qualified_ref_is_valid(value jsonb) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $_$
 SELECT COALESCE(jsonb_typeof(value) = 'object'
 AND value - ARRAY['registry_deployment','id','version']::text[] = '{}'::jsonb
 AND jsonb_typeof(value->'registry_deployment') = 'string' AND jsonb_typeof(value->'id') = 'string' AND jsonb_typeof(value->'version') = 'string'
 AND value->>'registry_deployment' ~ '^aidash://[A-Za-z0-9-]{1,100}$'
 AND value->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'
 AND value->>'version' ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?([+][0-9A-Za-z.-]+)?$', false)
$_$;

--

CREATE FUNCTION public.aidash_source_declaration_is_valid(value jsonb, operation text) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $$
DECLARE tools jsonb; pin jsonb; disclosure jsonb; candidate jsonb;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['provider','operation','tools','pin','volatility','query','refresh','disclosure','required_home']::text[] <> '{}'::jsonb
 OR value->>'provider' IS DISTINCT FROM 'aidash.native-context/1' OR value->>'operation' IS DISTINCT FROM operation THEN RETURN false; END IF;
 tools := value->'tools';
 IF jsonb_typeof(tools) IS DISTINCT FROM 'object' OR tools - ARRAY['read','write','detail']::text[] <> '{}'::jsonb
 OR jsonb_typeof(tools->'read') IS DISTINCT FROM 'array' OR jsonb_typeof(tools->'write') IS DISTINCT FROM 'array'
 OR jsonb_array_length(tools->'read')+jsonb_array_length(tools->'write') > 32 THEN RETURN false; END IF;
 FOR candidate IN SELECT jsonb_array_elements((tools->'read') || (tools->'write')) LOOP
  IF NOT COALESCE(public.aidash_qualified_ref_is_valid(candidate),false) THEN RETURN false; END IF;
 END LOOP;
 IF (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements(tools->'read') e)
 OR (SELECT count(*) <> count(DISTINCT e) FROM jsonb_array_elements(tools->'write') e) THEN RETURN false; END IF;
 IF COALESCE(tools->'detail','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_qualified_ref_is_valid(tools->'detail'),false) THEN RETURN false; END IF;
 IF value ? 'volatility' AND NOT COALESCE(value->>'volatility' IN ('run','step'),false) THEN RETURN false; END IF;
 IF value ? 'query' AND NOT COALESCE(value->>'query' IN ('task-inputs@1','accepted-inputs-first@1'),false) THEN RETURN false; END IF;
 IF value ? 'refresh' AND value->>'refresh' IS DISTINCT FROM 'revision-bound@1' THEN RETURN false; END IF;
 IF value ? 'required_home' AND jsonb_typeof(value->'required_home') <> 'boolean' THEN RETURN false; END IF;
 disclosure := COALESCE(value->'disclosure','{"previews":false,"query":false}'::jsonb);
 IF jsonb_typeof(disclosure) <> 'object' OR disclosure - ARRAY['previews','query']::text[] <> '{}'::jsonb
 OR jsonb_typeof(disclosure->'previews') IS DISTINCT FROM 'boolean' OR jsonb_typeof(disclosure->'query') IS DISTINCT FROM 'boolean' THEN RETURN false; END IF;
 pin := COALESCE(value->'pin','{"policy":"always"}'::jsonb);
 IF jsonb_typeof(pin) <> 'object' THEN RETURN false; END IF;
 IF pin->>'policy' IN ('always','on_demand') THEN RETURN pin - 'policy' = '{}'::jsonb; END IF;
 RETURN pin->>'policy' = 'scored' AND pin - ARRAY['policy','mode','decider']::text[] = '{}'::jsonb
 AND pin->>'mode' IN ('shadow','enforce') AND COALESCE(public.aidash_qualified_ref_is_valid(pin->'decider'),false)
 AND disclosure->'previews' = 'true'::jsonb AND disclosure->'query' = 'true'::jsonb
 AND NOT (COALESCE(value->'required_home','false'::jsonb) = 'true'::jsonb AND pin->>'mode' = 'enforce');
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

--

CREATE FUNCTION public.aidash_tool_config_is_valid(input_value jsonb) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $_$ SELECT (CASE input_value->>'transport' WHEN 'native' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','operation','allowed_hosts']::text[]) = '{}'::jsonb ELSE false END AND jsonb_typeof(input_value->'operation') = 'string' AND input_value->>'operation' IN ('echo', 'http_get') AND (NOT (input_value ? 'allowed_hosts') OR jsonb_typeof(input_value->'allowed_hosts') = 'array' AND NOT jsonb_path_exists(input_value->'allowed_hosts', 'strict $[*] ? (@.type() != "string")', '{}'::jsonb, true)) AND (input_value->>'operation' <> 'http_get' OR CASE WHEN jsonb_typeof(COALESCE(input_value->'allowed_hosts', '[]'::jsonb)) = 'array' THEN jsonb_array_length(COALESCE(input_value->'allowed_hosts', '[]'::jsonb)) > 0 ELSE false END)) WHEN 'http' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','endpoint','replay','secret','secret_owner']::text[]) = '{}'::jsonb ELSE false END AND aidash_valid_http_endpoint(input_value->'endpoint') AND (NOT (input_value ? 'secret_owner') OR input_value->'secret_owner' IN ('"any"'::jsonb, '"personal"'::jsonb)) AND (COALESCE(input_value->'secret','null'::jsonb) = 'null'::jsonb OR (jsonb_typeof(input_value->'secret') = 'string' AND input_value->>'secret' ~ '^[a-z0-9][a-z0-9_-]{0,63}$')) AND jsonb_typeof(input_value->'replay') = 'string' AND input_value->>'replay' IN ('read_only', 'idempotent', 'unsafe')) WHEN 'mcp' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','endpoint','tool_name','replay','idempotency_argument','secret','secret_owner','scopes']::text[]) = '{}'::jsonb ELSE false END AND aidash_valid_http_endpoint(input_value->'endpoint') AND (NOT (input_value ? 'secret_owner') OR input_value->'secret_owner' IN ('"any"'::jsonb, '"personal"'::jsonb)) AND (COALESCE(input_value->'secret','null'::jsonb) = 'null'::jsonb OR (jsonb_typeof(input_value->'secret') = 'string' AND input_value->>'secret' ~ '^[a-z0-9][a-z0-9_-]{0,63}$')) AND (CASE WHEN NOT (input_value ? 'scopes') THEN true WHEN jsonb_typeof(input_value->'scopes') = 'array' THEN NOT EXISTS (SELECT 1 FROM jsonb_array_elements(input_value->'scopes') AS scopes(item) WHERE jsonb_typeof(item) IS DISTINCT FROM 'string' OR length(btrim(item #>> '{}', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) = 0) ELSE false END) AND jsonb_typeof(input_value->'tool_name') = 'string' AND jsonb_typeof(input_value->'replay') = 'string' AND input_value->>'replay' IN ('read_only', 'idempotent', 'unsafe') AND jsonb_typeof(COALESCE(input_value->'idempotency_argument', 'null'::jsonb)) IN ('string', 'null') AND (input_value->>'replay' <> 'idempotent' OR (jsonb_typeof(input_value->'idempotency_argument') = 'string' AND length(btrim(input_value->>'idempotency_argument', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))) WHEN 'agent' THEN (CASE WHEN jsonb_typeof((input_value)) = 'object' THEN ((input_value) - ARRAY['transport','node_id','agent']::text[]) = '{}'::jsonb ELSE false END AND jsonb_typeof(input_value->'node_id') = 'string' AND input_value->>'node_id' ~ '^aidash://[A-Za-z0-9-]{1,100}$' AND jsonb_typeof(input_value->'agent') = 'object' AND jsonb_typeof(input_value->'agent'->'id') = 'string' AND jsonb_typeof(input_value->'agent'->'version') = 'string' AND input_value->'agent'->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$' AND CASE WHEN input_value->'agent'->>'version' ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$' THEN split_part(input_value->'agent'->>'version', '.', 1)::numeric <= 18446744073709551615 AND split_part(input_value->'agent'->>'version', '.', 2)::numeric <= 18446744073709551615 AND split_part(split_part(split_part(input_value->'agent'->>'version', '.', 3), '-', 1), '+', 1)::numeric <= 18446744073709551615 ELSE false END) ELSE false END) $_$;

--

CREATE FUNCTION public.aidash_valid_http_endpoint(input_value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE
    AS $_$
DECLARE
    endpoint text;
    authority text;
    host text;
    port_text text;
BEGIN
    IF input_value IS NULL OR jsonb_typeof(input_value) <> 'string' THEN
        RETURN false;
    END IF;
    endpoint := input_value #>> '{}';
    IF endpoint !~* '^https?://' OR strpos(endpoint, '?') > 0 OR strpos(endpoint, '#') > 0
       OR endpoint ~ '[[:cntrl:]]'
       OR endpoint ~ '%([^0-9A-Fa-f]|$)'
       OR endpoint ~ '%[0-9A-Fa-f]([^0-9A-Fa-f]|$)' THEN
        RETURN false;
    END IF;

    endpoint := regexp_replace(endpoint, '^https?://', '', 'i');
    authority := split_part(endpoint, '/', 1);
    IF authority = '' OR authority ~ '[@[:space:]]' THEN
        RETURN false;
    END IF;

    IF left(authority, 1) = '[' THEN
        IF authority !~ '^\[[0-9A-Fa-f:.]+\](:[0-9]*)?$' THEN
            RETURN false;
        END IF;
        host := substring(authority FROM 2 FOR strpos(authority, ']') - 2);
        IF strpos(host, ':') = 0 THEN
            RETURN false;
        END IF;
        PERFORM host::inet;
        IF authority ~ ':[0-9]+$' THEN
            port_text := regexp_replace(authority, '^.*:', '');
        END IF;
    ELSE
        IF authority ~ ':[0-9]*$' THEN
            port_text := regexp_replace(authority, '^.*:', '');
            IF port_text = '' THEN
                port_text := NULL;
            END IF;
            host := regexp_replace(authority, ':[0-9]*$', '');
        ELSE
            host := authority;
        END IF;
        IF host !~ '^([[:alnum:]_]([[:alnum:]_-]*[[:alnum:]_])?)(\.([[:alnum:]_]([[:alnum:]_-]*[[:alnum:]_])?))*\.?$' THEN
            RETURN false;
        END IF;
        IF host ~ '^[0-9.]+$' THEN
            -- Do not let an invalid numeric IPv4 literal pass as a DNS name.
            PERFORM host::inet;
        END IF;
    END IF;

    IF port_text IS NOT NULL AND port_text::numeric NOT BETWEEN 0 AND 65535 THEN
        RETURN false;
    END IF;
    RETURN true;
EXCEPTION WHEN OTHERS THEN
    RETURN false;
END
$_$;

--

CREATE FUNCTION public.guard_installation_config() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
DECLARE
    target_kind text;
    target_config jsonb;
BEGIN
    SELECT kind, metadata->'config' INTO target_kind, target_config
    FROM registry
    WHERE id = NEW.id AND version = NEW.version;

    IF target_kind = 'model' AND NOT COALESCE(
        (CASE WHEN (NEW.config->'request_timeout_secs') IS NULL OR (NEW.config->'request_timeout_secs') = 'null'::jsonb THEN true WHEN jsonb_typeof(NEW.config->'request_timeout_secs') = 'number' AND (NEW.config->'request_timeout_secs')::text ~ '^[1-9][0-9]*$' THEN (NEW.config->'request_timeout_secs')::text::numeric BETWEEN 1 AND 4294967295 ELSE false END) AND
        (CASE WHEN (NEW.config->'stream_stall_timeout_secs') IS NULL OR (NEW.config->'stream_stall_timeout_secs') = 'null'::jsonb THEN true WHEN jsonb_typeof(NEW.config->'stream_stall_timeout_secs') = 'number' AND (NEW.config->'stream_stall_timeout_secs')::text ~ '^[1-9][0-9]*$' THEN (NEW.config->'stream_stall_timeout_secs')::text::numeric BETWEEN 1 AND 4294967295 ELSE false END) AND
        (NOT NEW.config ? 'streaming' OR jsonb_typeof(NEW.config->'streaming') IN ('boolean','null')) AND
        (NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','streaming','stream_stall_timeout_secs','media_routes']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'provider' OR (jsonb_typeof(NEW.config->'provider') = 'string' AND NEW.config->>'provider' = 'openrouter'))
        AND (NOT NEW.config ? 'model_id' OR (jsonb_typeof(NEW.config->'model_id') = 'string' AND length(btrim(NEW.config->>'model_id', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND (NOT NEW.config ? 'endpoint' OR (jsonb_typeof(NEW.config->'endpoint') = 'string' AND length(btrim(NEW.config->>'endpoint', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND NOT NEW.config ? 'credential_env'
        AND (NOT NEW.config ? 'reasoning_effort' OR NEW.config->'reasoning_effort' = 'null'::jsonb OR NEW.config->>'reasoning_effort' IN ('none','minimal','low','medium','high','xhigh','max'))
        AND (NOT NEW.config ? 'context_window' OR (jsonb_typeof(NEW.config->'context_window') = 'number' AND NEW.config->>'context_window' ~ '^(0|[1-9][0-9]*)$' AND (NEW.config->>'context_window')::numeric >= 2048 AND (NEW.config->>'context_window')::numeric <= 18446744073709551615))
        AND (NOT NEW.config ? 'media_routes' OR aidash_media_routes_valid(NEW.config->'media_routes'))
        AND (NOT NEW.config ? 'modalities' OR (jsonb_typeof(NEW.config->'modalities') = 'array' AND NEW.config->'modalities' @> '["text"]'::jsonb AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.config->'modalities') AS item WHERE jsonb_typeof(item) <> 'string')))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid model configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'agent' AND NOT COALESCE(
        NEW.config - ARRAY['model','instructions','bindings','remove_default','cluster','max_steps','exposure','tool_parallelism']::text[] = '{}'::jsonb
        AND public.aidash_agent_bindings_is_valid(target_config || NEW.config), false) THEN
        RAISE EXCEPTION 'installation override is not a valid Binding configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'agent' AND NEW.config ? 'model' AND NOT EXISTS (
        SELECT 1 FROM registry model_record
        WHERE model_record.id = NEW.config#>>'{model,id}'
          AND model_record.version = NEW.config#>>'{model,version}'
          AND model_record.kind = 'model'
    ) THEN
        RAISE EXCEPTION 'installed agent model override must reference a registered model'
            USING ERRCODE = '23514', CONSTRAINT = 'registry_agent_model_installation_reference';
    END IF;

    IF target_kind = 'cluster' AND NOT COALESCE(
        (NEW.config - ARRAY['coordinator']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'coordinator' OR (jsonb_typeof(NEW.config->'coordinator') = 'object' AND jsonb_typeof(NEW.config->'coordinator'->'id') = 'string' AND jsonb_typeof(NEW.config->'coordinator'->'version') = 'string'))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid cluster configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'skill' AND NOT COALESCE(
        (NEW.config - ARRAY['instructions']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'instructions' OR (jsonb_typeof(NEW.config->'instructions') = 'string' AND length(btrim(NEW.config->>'instructions', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid skill configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'tool' AND NOT COALESCE(
        NEW.config - ARRAY['transport','narrow']::text[] = '{}'::jsonb AND public.aidash_descriptor_is_valid(target_config || NEW.config), false
    ) THEN
        RAISE EXCEPTION 'installation override is not a valid tool configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    RETURN NEW;
END $_$;

--

CREATE FUNCTION public.guard_registry_agent_model_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        IF NOT public.aidash_tenant_purge() THEN
            RAISE EXCEPTION 'registry_agent_model_refs is maintained by registry'
                USING ERRCODE = '23514', CONSTRAINT = 'registry_agent_model_reference';
        END IF;
    END IF;
    RETURN NULL;
END $$;

--

CREATE FUNCTION public.guard_registry_agent_resource_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        IF NOT public.aidash_tenant_purge() THEN
            RAISE EXCEPTION 'registry_agent_resource_refs is maintained by registry'
                USING ERRCODE = '23514', CONSTRAINT = 'registry_agent_resource_reference';
        END IF;
    END IF;
    RETURN NULL;
END $$;

--

CREATE FUNCTION public.guard_registry_memory_role_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF pg_trigger_depth() < 2 THEN
  IF NOT public.aidash_tenant_purge() THEN
   RAISE EXCEPTION 'memory role references are maintained by registry'
    USING ERRCODE = '23514', CONSTRAINT = 'registry_memory_role_reference';
  END IF;
 END IF;
 RETURN NULL;
END $$;

--

CREATE FUNCTION public.lock_registry_installation_writes() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Acquire one schema-scoped lock before either table takes row locks. This
    -- gives base-record edits and installation edits a consistent lock order.
    PERFORM pg_advisory_xact_lock(70721023, hashtext(TG_TABLE_SCHEMA));
    RETURN NULL;
END $$;

--

CREATE FUNCTION public.marketplace_registry_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.metadata ? 'installation' OR (TG_OP='UPDATE' AND OLD.metadata ? 'installation') OR EXISTS (SELECT 1 FROM registry WHERE tenant=NEW.tenant AND id=NEW.id AND version=NEW.version AND metadata ? 'installation') THEN
  IF TG_OP <> 'INSERT' OR current_setting('aidash.marketplace_writer', true) IS DISTINCT FROM '1'
    OR NEW.metadata->'installation'->>'contract' IS DISTINCT FROM '1'
    OR NEW.id NOT LIKE 'mkt-%' THEN
   RAISE EXCEPTION 'unsupported Marketplace writer';
  END IF;
 END IF;
 RETURN NEW;
END $$;

--

CREATE FUNCTION public.sync_registry_agent_model_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.kind = 'agent' THEN
        DELETE FROM registry_agent_model_refs
        WHERE tenant = OLD.tenant AND agent_id = OLD.id AND agent_version = OLD.version;
    END IF;
    IF NEW.kind = 'agent' THEN
        INSERT INTO registry_agent_model_refs(tenant, agent_id, agent_version, model_id, model_version)
        VALUES (NEW.tenant, NEW.id, NEW.version, NEW.metadata#>>'{config,model,id}', NEW.metadata#>>'{config,model,version}');
    END IF;
    RETURN NEW;
END $$;

--

CREATE FUNCTION public.sync_registry_agent_resource_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.kind = 'agent' THEN
        DELETE FROM registry_agent_resource_refs
        WHERE tenant = OLD.tenant AND agent_id = OLD.id AND agent_version = OLD.version;
    END IF;
    IF NEW.kind = 'agent' THEN
        INSERT INTO registry_agent_resource_refs(
            tenant, agent_id, agent_version, required_kind, ordinal, reference_id, reference_version
        )
        SELECT NEW.tenant, NEW.id, NEW.version, 'tool', reference.ordinality::integer,
               reference.value->>'id', reference.value->>'version'
        FROM jsonb_array_elements(COALESCE(NEW.metadata#>'{config,tools}', '[]'::jsonb))
             WITH ORDINALITY AS reference(value, ordinality);
        INSERT INTO registry_agent_resource_refs(
            tenant, agent_id, agent_version, required_kind, ordinal, reference_id, reference_version
        )
        SELECT NEW.tenant, NEW.id, NEW.version, 'skill', reference.ordinality::integer,
               reference.value->>'id', reference.value->>'version'
        FROM jsonb_array_elements(COALESCE(NEW.metadata#>'{config,skills}', '[]'::jsonb))
             WITH ORDINALITY AS reference(value, ordinality);
        IF jsonb_typeof(NEW.metadata#>'{config,cluster}') = 'object' THEN
            INSERT INTO registry_agent_resource_refs(
                tenant, agent_id, agent_version, required_kind, ordinal, reference_id, reference_version
            )
            VALUES (
                NEW.tenant, NEW.id, NEW.version, 'cluster', 1,
                NEW.metadata#>>'{config,cluster,id}',
                NEW.metadata#>>'{config,cluster,version}'
            );
        END IF;
    END IF;
    RETURN NEW;
END $$;

--

CREATE FUNCTION public.sync_registry_memory_role_refs() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE config jsonb; binding jsonb; role text; expected text;
BEGIN
 IF TG_OP = 'UPDATE' THEN
  DELETE FROM registry_memory_role_refs WHERE tenant = OLD.tenant AND source_id = OLD.id AND source_version = OLD.version;
 END IF;
 config := NEW.metadata->'config';
 IF NEW.kind = 'agent' THEN
  INSERT INTO registry_memory_role_refs
   SELECT NEW.tenant, NEW.id, NEW.version, value->>'kind', ordinal::integer, value->>'kind', value#>>'{target,id}', value#>>'{target,version}'
   FROM jsonb_array_elements(COALESCE(config->'bindings','[]'::jsonb)) WITH ORDINALITY AS ref(value, ordinal)
   WHERE value->>'kind' IN ('memory','source');
 ELSIF NEW.kind = 'memory' AND config ? 'engine' THEN
  FOREACH role IN ARRAY ARRAY['extraction','derivation','reflection','embedding','reranker','tokenizer'] LOOP
   expected := CASE WHEN role IN ('extraction','derivation','reflection') THEN 'model' ELSE role END;
   binding := config->'policy'->role;
   INSERT INTO registry_memory_role_refs VALUES (NEW.tenant, NEW.id, NEW.version, role, 1, expected, binding->>'id', binding->>'version');
  END LOOP;
 ELSIF NEW.kind = 'source' AND config ? 'scope' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.tenant, NEW.id, NEW.version, 'memory', 1, 'memory', config#>>'{memory,id}', config#>>'{memory,version}');
 ELSIF NEW.kind = 'reranker' AND config->>'provider' = 'model' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.tenant, NEW.id, NEW.version, 'model', 1, 'model', config#>>'{model,id}', config#>>'{model,version}');
 END IF;
 RETURN NEW;
END $$;

--

CREATE FUNCTION public.validate_registry_installations() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Re-run the same effective-config validation when a base registry record
    -- changes; otherwise previously-valid overrides can become invalid.
    IF TG_OP = 'UPDATE' THEN
        UPDATE installations SET config = config
        WHERE id = NEW.id AND version = NEW.version;
    END IF;
    -- Model overrides are independent registry references. Revalidate them
    -- when their target is updated or deleted as well.
    UPDATE installations AS installed SET config = installed.config
    FROM registry AS agent_record
    WHERE agent_record.id = installed.id
      AND agent_record.version = installed.version
      AND agent_record.kind = 'agent'
      AND installed.config#>>'{model,id}' = OLD.id
      AND installed.config#>>'{model,version}' = OLD.version;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;



SET default_tablespace = '';

SET default_table_access_method = heap;

--

CREATE TABLE public.agent_draft_registrations (
    tenant text NOT NULL,
    draft_id uuid NOT NULL,
    revision bigint NOT NULL,
    agent_id text NOT NULL,
    version text NOT NULL,
    actor text NOT NULL,
    release_notes text DEFAULT ''::text NOT NULL,
    source_id text,
    source_version text,
    behavioral_tested boolean DEFAULT false NOT NULL,
    registered_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.agent_draft_registrations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_draft_shares (
    draft_id uuid NOT NULL,
    tenant text NOT NULL,
    subject text NOT NULL,
    can_edit boolean DEFAULT false NOT NULL,
    documents_digest text NOT NULL
);

ALTER TABLE ONLY public.agent_draft_shares FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_drafts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    owner text NOT NULL,
    managed_id text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    entry jsonb NOT NULL,
    documents jsonb NOT NULL,
    release_notes text DEFAULT ''::text NOT NULL,
    source_id text,
    source_version text,
    archived boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.agent_drafts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_incident_events (
    id bigint NOT NULL,
    tenant text NOT NULL,
    incident_id uuid NOT NULL,
    actor text NOT NULL,
    change jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.agent_incident_events FORCE ROW LEVEL SECURITY;

--

CREATE SEQUENCE public.agent_incident_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--

ALTER SEQUENCE public.agent_incident_events_id_seq OWNED BY public.agent_incident_events.id;

--

CREATE TABLE public.agent_incidents (
    id uuid NOT NULL,
    tenant text NOT NULL,
    agent_id text NOT NULL,
    version text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    severity text NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    owner text NOT NULL,
    notes text NOT NULL,
    evidence jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    resolved_at timestamp with time zone,
    evidence_expires_at timestamp with time zone,
    evidence_expired_at timestamp with time zone
);

ALTER TABLE ONLY public.agent_incidents FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_knowledge (
    tenant text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    documents jsonb NOT NULL,
    image_index jsonb
);

ALTER TABLE ONLY public.agent_knowledge FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_knowledge_images (
    tenant text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    document_index integer NOT NULL,
    media_type text NOT NULL,
    size bigint NOT NULL,
    sha256 text NOT NULL,
    bytes bytea NOT NULL,
    CONSTRAINT agent_knowledge_images_metadata CHECK (((document_index >= 0) AND (media_type ~~ 'image/%'::text) AND (size = octet_length(bytes)) AND (sha256 ~ '^[0-9a-f]{64}$'::text)))
);

ALTER TABLE ONLY public.agent_knowledge_images FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_plugin_copies (
    tenant text NOT NULL,
    digest text NOT NULL,
    name text NOT NULL,
    source text NOT NULL,
    plugin jsonb NOT NULL,
    files jsonb NOT NULL,
    denied jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    entitlement_expires_at timestamp with time zone,
    CONSTRAINT agent_plugin_copies_digest CHECK ((digest ~ '^sha256:[0-9a-f]{64}$'::text)),
    CONSTRAINT agent_plugin_copies_documents CHECK (((jsonb_typeof(plugin) = 'object'::text) AND ((plugin ->> 'digest'::text) = digest) AND (jsonb_typeof(files) = 'array'::text) AND (jsonb_typeof(denied) = 'array'::text)))
);

ALTER TABLE ONLY public.agent_plugin_copies FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_test_limits (
    tenant text NOT NULL,
    max_input_bytes integer DEFAULT 20000 NOT NULL,
    max_output_tokens integer DEFAULT 2048 NOT NULL,
    max_total_tokens integer DEFAULT 16384 NOT NULL,
    max_steps integer DEFAULT 12 NOT NULL,
    max_duration_secs integer DEFAULT 60 NOT NULL,
    max_concurrent integer DEFAULT 2 NOT NULL,
    payload_days integer DEFAULT 30 NOT NULL,
    incident_evidence_days integer DEFAULT 90 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL
);

ALTER TABLE ONLY public.agent_test_limits FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_test_profiles (
    tenant text NOT NULL,
    id text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    rules jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.agent_test_profiles FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.agent_test_sessions (
    id uuid NOT NULL,
    draft_id uuid NOT NULL,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    status text NOT NULL,
    scenario jsonb NOT NULL,
    conversation jsonb,
    tool_calls jsonb,
    usage jsonb NOT NULL,
    error text,
    active_slot uuid,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    expired_at timestamp with time zone
);

ALTER TABLE ONLY public.agent_test_sessions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.marketplace_plugin_recalls (
    plugin_id uuid NOT NULL,
    version text NOT NULL,
    digest text NOT NULL,
    reason text NOT NULL,
    recalled_at timestamp with time zone NOT NULL,
    CONSTRAINT marketplace_plugin_recalls_digest CHECK ((digest ~ '^sha256:[0-9a-f]{64}$'::text))
);

--

CREATE TABLE public.registry (
    tenant text NOT NULL,
    id text NOT NULL,
    version text NOT NULL,
    kind text NOT NULL,
    metadata jsonb NOT NULL,
    CONSTRAINT registry_cluster_config CHECK (COALESCE((true AND ((kind <> 'cluster'::text) OR
CASE
    WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['coordinator'::text]) = '{}'::jsonb)
    ELSE false
END) AND ((kind <> 'cluster'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'coordinator'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'coordinator'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'coordinator'::text) -> 'version'::text)) = 'string'::text) AND ((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'id'::text) ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text) AND
CASE
    WHEN ((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text) ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part((((metadata -> 'config'::text) -> 'coordinator'::text) ->> 'version'::text), '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
    ELSE false
END))), false)),
    CONSTRAINT registry_embedding_config CHECK (COALESCE((true AND ((kind <> 'embedding'::text) OR (
CASE
    WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text, 'provider_credential'::text, 'model'::text, 'model_version'::text, 'dimensions'::text, 'inputs'::text]) = '{}'::jsonb)
    ELSE false
END AND (jsonb_typeof(((metadata -> 'config'::text) -> 'provider'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'provider'::text) = ANY (ARRAY['openai'::text, 'openrouter'::text])) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'endpoint'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'endpoint'::text) ~ '^https?://[^/@?#[:space:]]+'::text) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'credential_env'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'credential_env'::text), 'null'::jsonb)) <> 'string'::text) OR (((metadata -> 'config'::text) ->> 'credential_env'::text) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text)) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model'::text), E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model'::text)) <= 256) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model_version'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model_version'::text), E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model_version'::text)) <= 128) AND
CASE
    WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'dimensions'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'dimensions'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'dimensions'::text))::text)::numeric >= (1)::numeric) AND (((((metadata -> 'config'::text) -> 'dimensions'::text))::text)::numeric <= (8192)::numeric))
    ELSE false
END))), false)),
    CONSTRAINT registry_embedding_inputs CHECK (((kind <> 'embedding'::text) OR (((metadata #> '{config,inputs}'::text[]) IS NULL) OR ((metadata #> '{config,inputs}'::text[]) = ANY (ARRAY['["text"]'::jsonb, '["text", "image"]'::jsonb, '["image", "text"]'::jsonb]))))),
    CONSTRAINT registry_identity CHECK (COALESCE(((id ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$'::text) AND ((metadata -> 'id'::text) = to_jsonb(id)) AND ((metadata -> 'version'::text) = to_jsonb(version)) AND ((metadata -> 'kind'::text) = to_jsonb(kind)) AND ((kind <> 'agent'::text) OR ((octet_length(id) + octet_length(version)) <= 139))), false)),
    CONSTRAINT registry_kind_check CHECK ((kind = ANY (ARRAY['agent'::text, 'bundle'::text, 'cluster'::text, 'decider'::text, 'embedding'::text, 'external_service'::text, 'memory'::text, 'model'::text, 'node'::text, 'reranker'::text, 'skill'::text, 'source'::text, 'tokenizer'::text, 'tool'::text]))),
    CONSTRAINT registry_kind_model_enum_check CHECK ((kind = ANY (ARRAY['agent'::text, 'bundle'::text, 'cluster'::text, 'decider'::text, 'embedding'::text, 'external_service'::text, 'memory'::text, 'model'::text, 'node'::text, 'reranker'::text, 'skill'::text, 'source'::text, 'tokenizer'::text, 'tool'::text]))),
    CONSTRAINT registry_model_cache_mode CHECK (((kind <> 'model'::text) OR (NOT ((metadata -> 'config'::text) ? 'cache_mode'::text)) OR ((metadata #> '{config,cache_mode}'::text[]) = ANY (ARRAY['"none"'::jsonb, '"automatic"'::jsonb, '"explicit"'::jsonb])))),
    CONSTRAINT registry_model_config CHECK (COALESCE((((kind <> 'model'::text) OR (((metadata #>> '{config,provider}'::text[]) = ANY (ARRAY['openrouter'::text, 'chatgpt'::text])) AND (jsonb_typeof((metadata #> '{config,model_id}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,model_id}'::text[]), E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND public.aidash_valid_http_endpoint((metadata #> '{config,endpoint}'::text[])) AND
CASE
    WHEN (jsonb_typeof((metadata #> '{config,context_window}'::text[])) = 'number'::text) THEN ((((metadata #>> '{config,context_window}'::text[]))::numeric >= (2048)::numeric) AND (trunc(((metadata #>> '{config,context_window}'::text[]))::numeric) = ((metadata #>> '{config,context_window}'::text[]))::numeric))
    ELSE false
END AND (jsonb_typeof((metadata #> '{config,modalities}'::text[])) = 'array'::text) AND ((metadata #> '{config,modalities}'::text[]) @> '["text"]'::jsonb) AND ((NOT ((metadata -> 'config'::text) ? 'reasoning_effort'::text)) OR ((metadata #> '{config,reasoning_effort}'::text[]) = 'null'::jsonb) OR ((metadata #>> '{config,reasoning_effort}'::text[]) = ANY (ARRAY['none'::text, 'minimal'::text, 'low'::text, 'medium'::text, 'high'::text, 'xhigh'::text, 'max'::text]))))) AND ((kind <> 'model'::text) OR (
CASE
    WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'model_id'::text, 'endpoint'::text, 'credential_env'::text, 'provider_credential'::text, 'plan_connection'::text, 'reasoning_effort'::text, 'context_window'::text, 'max_output_tokens'::text, 'modalities'::text, 'cost'::text, 'request_timeout_secs'::text, 'streaming'::text, 'stream_stall_timeout_secs'::text, 'media_routes'::text, 'projection_versions'::text, 'cache_mode'::text, 'request_estimator'::text, 'provider_route'::text, 'estimate_diagnostics'::text]) = '{}'::jsonb)
    ELSE false
END AND ((metadata -> 'config'::text) ? 'cost'::text) AND (jsonb_typeof(COALESCE((metadata #> '{config,credential_env}'::text[]), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND (jsonb_typeof((metadata #> '{config,modalities}'::text[])) = 'array'::text) AND (NOT jsonb_path_exists((metadata #> '{config,modalities}'::text[]), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND
CASE
    WHEN ((jsonb_typeof((metadata #> '{config,context_window}'::text[])) = 'number'::text) AND (((metadata #> '{config,context_window}'::text[]))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((metadata #> '{config,context_window}'::text[]))::text)::numeric <= '18446744073709551615'::numeric)
    ELSE false
END AND
CASE
    WHEN ((NOT ((metadata -> 'config'::text) ? 'max_output_tokens'::text)) OR (((metadata -> 'config'::text) -> 'max_output_tokens'::text) = 'null'::jsonb)) THEN true
    WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_output_tokens'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'max_output_tokens'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN
    CASE
        WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'context_window'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'context_window'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((metadata -> 'config'::text) ->> 'max_output_tokens'::text))::numeric >= (1)::numeric) AND ((((metadata -> 'config'::text) ->> 'max_output_tokens'::text))::numeric <= LEAST(('4294967295'::bigint)::numeric, (((metadata -> 'config'::text) ->> 'context_window'::text))::numeric)))
        ELSE false
    END
    ELSE false
END)) AND ((kind <> 'model'::text) OR
CASE
    WHEN (((metadata #> '{config,request_timeout_secs}'::text[]) IS NULL) OR ((metadata #> '{config,request_timeout_secs}'::text[]) = 'null'::jsonb)) THEN true
    WHEN ((jsonb_typeof((metadata #> '{config,request_timeout_secs}'::text[])) = 'number'::text) AND (((metadata #> '{config,request_timeout_secs}'::text[]))::text ~ '^[1-9][0-9]*$'::text)) THEN (((((metadata #> '{config,request_timeout_secs}'::text[]))::text)::numeric >= (1)::numeric) AND ((((metadata #> '{config,request_timeout_secs}'::text[]))::text)::numeric <= ('4294967295'::bigint)::numeric))
    ELSE false
END) AND ((kind <> 'model'::text) OR ((NOT ((metadata -> 'config'::text) ? 'media_routes'::text)) OR public.aidash_media_routes_valid((metadata #> '{config,media_routes}'::text[])))) AND ((kind <> 'model'::text) OR (NOT ((metadata -> 'config'::text) ? 'projection_versions'::text)) OR ((jsonb_typeof((metadata #> '{config,projection_versions}'::text[])) = 'array'::text) AND ('["legacy", "ordered", "native"]'::jsonb @> (metadata #> '{config,projection_versions}'::text[]))))), false)),
    CONSTRAINT registry_model_projection_measurement CHECK (((kind <> 'model'::text) OR COALESCE((((NOT ((metadata -> 'config'::text) ? 'provider_route'::text)) OR ((metadata #> '{config,provider_route}'::text[]) = 'null'::jsonb) OR (((metadata #>> '{config,provider}'::text[]) = 'openrouter'::text) AND (COALESCE((metadata #> '{config,plan_connection}'::text[]), 'null'::jsonb) = 'null'::jsonb) AND public.aidash_provider_route_valid((metadata #> '{config,provider_route}'::text[])))) AND ((NOT ((metadata -> 'config'::text) ? 'estimate_diagnostics'::text)) OR ((metadata #> '{config,estimate_diagnostics}'::text[]) = 'null'::jsonb) OR public.aidash_estimate_diagnostics_valid((metadata #> '{config,estimate_diagnostics}'::text[])))), false))),
    CONSTRAINT registry_model_provider_keys CHECK (((kind <> ALL (ARRAY['model'::text, 'embedding'::text])) OR (NOT ((metadata -> 'config'::text) ? 'credential_env'::text)))),
    CONSTRAINT registry_model_request_estimator CHECK (((kind <> 'model'::text) OR (NOT ((metadata -> 'config'::text) ? 'request_estimator'::text)) OR ((metadata #> '{config,request_estimator}'::text[]) = ANY (ARRAY['"bytes"'::jsonb, '"calibrated"'::jsonb])))),
    CONSTRAINT registry_model_streaming CHECK (((kind <> 'model'::text) OR (
CASE
    WHEN (((metadata #> '{config,streaming}'::text[]) IS NULL) OR ((metadata #> '{config,streaming}'::text[]) = 'null'::jsonb)) THEN true
    ELSE (jsonb_typeof((metadata #> '{config,streaming}'::text[])) = 'boolean'::text)
END AND
CASE
    WHEN (((metadata #> '{config,stream_stall_timeout_secs}'::text[]) IS NULL) OR ((metadata #> '{config,stream_stall_timeout_secs}'::text[]) = 'null'::jsonb)) THEN true
    WHEN ((jsonb_typeof((metadata #> '{config,stream_stall_timeout_secs}'::text[])) = 'number'::text) AND (((metadata #> '{config,stream_stall_timeout_secs}'::text[]))::text ~ '^[1-9][0-9]*$'::text)) THEN (((((metadata #> '{config,stream_stall_timeout_secs}'::text[]))::text)::numeric >= (1)::numeric) AND ((((metadata #> '{config,stream_stall_timeout_secs}'::text[]))::text)::numeric <= ('4294967295'::bigint)::numeric))
    ELSE false
END))),
    CONSTRAINT registry_plan_connection_config CHECK (((kind <> 'model'::text) OR COALESCE(
CASE
    WHEN (((metadata #> '{config,plan_connection}'::text[]) IS NULL) OR ((metadata #> '{config,plan_connection}'::text[]) = 'null'::jsonb)) THEN ((metadata #>> '{config,provider}'::text[]) IS DISTINCT FROM 'chatgpt'::text)
    ELSE (((metadata #> '{config,plan_connection}'::text[]) = '"chatgpt"'::jsonb) AND ((metadata #>> '{config,provider}'::text[]) = 'chatgpt'::text) AND ((metadata #>> '{config,endpoint}'::text[]) = 'https://api.openai.com/v1'::text) AND (((metadata #> '{config,provider_credential}'::text[]) IS NULL) OR ((metadata #> '{config,provider_credential}'::text[]) = 'null'::jsonb)))
END, false))),
    CONSTRAINT registry_provider_credential_config CHECK (((kind <> ALL (ARRAY['model'::text, 'embedding'::text])) OR COALESCE((((metadata #> '{config,provider_credential}'::text[]) IS NULL) OR ((metadata #> '{config,provider_credential}'::text[]) = 'null'::jsonb) OR (((metadata #> '{config,provider_credential}'::text[]) = '"openrouter"'::jsonb) AND ((metadata #>> '{config,provider}'::text[]) = 'openrouter'::text) AND ((metadata #>> '{config,endpoint}'::text[]) = 'https://openrouter.ai/api/v1'::text) AND (((metadata #> '{config,credential_env}'::text[]) IS NULL) OR ((metadata #> '{config,credential_env}'::text[]) = 'null'::jsonb)))), false))),
    CONSTRAINT registry_semver CHECK (COALESCE(((version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) AND
CASE
    WHEN (version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'::text) THEN (((split_part(version, '.'::text, 1))::numeric <= '18446744073709551615'::numeric) AND ((split_part(version, '.'::text, 2))::numeric <= '18446744073709551615'::numeric) AND ((split_part(split_part(split_part(version, '.'::text, 3), '-'::text, 1), '+'::text, 1))::numeric <= '18446744073709551615'::numeric))
    ELSE false
END), false)),
    CONSTRAINT registry_skill_config CHECK (COALESCE(((kind <> 'skill'::text) OR ((jsonb_typeof((metadata #> '{config,instructions}'::text[])) = 'string'::text) AND (length(btrim((metadata #>> '{config,instructions}'::text[]), E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0))), false))
);

ALTER TABLE ONLY public.registry FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.registry_agent_model_refs (
    tenant text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    model_id text NOT NULL,
    model_version text NOT NULL,
    model_kind text DEFAULT 'model'::text NOT NULL,
    CONSTRAINT registry_agent_model_refs_model_kind_check CHECK ((model_kind = 'model'::text))
);

ALTER TABLE ONLY public.registry_agent_model_refs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.registry_agent_resource_refs (
    tenant text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    required_kind text NOT NULL,
    ordinal integer NOT NULL,
    reference_id text NOT NULL,
    reference_version text NOT NULL,
    CONSTRAINT registry_agent_resource_refs_required_kind_check CHECK ((required_kind = ANY (ARRAY['tool'::text, 'skill'::text, 'cluster'::text])))
);

ALTER TABLE ONLY public.registry_agent_resource_refs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.registry_memory_role_refs (
    tenant text NOT NULL,
    source_id text NOT NULL,
    source_version text NOT NULL,
    role text NOT NULL,
    ordinal integer NOT NULL,
    required_kind text NOT NULL,
    reference_id text NOT NULL,
    reference_version text NOT NULL
);

ALTER TABLE ONLY public.registry_memory_role_refs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.registry_requests (
    principal text NOT NULL,
    tenant text NOT NULL,
    key uuid NOT NULL,
    request jsonb NOT NULL,
    entity_id text NOT NULL
);

ALTER TABLE ONLY public.registry_requests FORCE ROW LEVEL SECURITY;

--

ALTER TABLE ONLY public.agent_incident_events ALTER COLUMN id SET DEFAULT nextval('public.agent_incident_events_id_seq'::regclass);

--

ALTER TABLE ONLY public.agent_draft_registrations
    ADD CONSTRAINT agent_draft_registrations_pkey PRIMARY KEY (draft_id, revision);

--

ALTER TABLE ONLY public.agent_draft_shares
    ADD CONSTRAINT agent_draft_shares_pkey PRIMARY KEY (draft_id, subject);

--

ALTER TABLE ONLY public.agent_drafts
    ADD CONSTRAINT agent_drafts_managed_id_key UNIQUE (managed_id);

--

ALTER TABLE ONLY public.agent_drafts
    ADD CONSTRAINT agent_drafts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.agent_drafts
    ADD CONSTRAINT agent_drafts_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.agent_incident_events
    ADD CONSTRAINT agent_incident_events_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.agent_incidents
    ADD CONSTRAINT agent_incidents_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.agent_incidents
    ADD CONSTRAINT agent_incidents_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.agent_knowledge_images
    ADD CONSTRAINT agent_knowledge_images_pkey PRIMARY KEY (tenant, agent_id, agent_version, document_index);

--

ALTER TABLE ONLY public.agent_knowledge
    ADD CONSTRAINT agent_knowledge_pkey PRIMARY KEY (tenant, agent_id, agent_version);

--

ALTER TABLE ONLY public.agent_plugin_copies
    ADD CONSTRAINT agent_plugin_copies_pkey PRIMARY KEY (tenant, digest);

--

ALTER TABLE ONLY public.agent_test_limits
    ADD CONSTRAINT agent_test_limits_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.agent_test_profiles
    ADD CONSTRAINT agent_test_profiles_pkey PRIMARY KEY (tenant, id);

--

ALTER TABLE ONLY public.agent_test_sessions
    ADD CONSTRAINT agent_test_sessions_active_slot_key UNIQUE (active_slot);

--

ALTER TABLE ONLY public.agent_test_sessions
    ADD CONSTRAINT agent_test_sessions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.marketplace_plugin_recalls
    ADD CONSTRAINT marketplace_plugin_recalls_pkey PRIMARY KEY (plugin_id, version);

--

ALTER TABLE public.registry
    ADD CONSTRAINT registry_agent_config CHECK (((kind <> 'agent'::text) OR COALESCE(public.aidash_agent_bindings_is_valid((metadata -> 'config'::text)), false))) NOT VALID;

--

ALTER TABLE ONLY public.registry_agent_model_refs
    ADD CONSTRAINT registry_agent_model_refs_pkey PRIMARY KEY (tenant, agent_id, agent_version);

--

ALTER TABLE ONLY public.registry_agent_resource_refs
    ADD CONSTRAINT registry_agent_resource_refs_pkey PRIMARY KEY (tenant, agent_id, agent_version, required_kind, ordinal);

--

ALTER TABLE public.registry
    ADD CONSTRAINT registry_bundle_config CHECK (((kind <> 'bundle'::text) OR COALESCE(public.aidash_bundle_is_valid((metadata -> 'config'::text)), false))) NOT VALID;

--

ALTER TABLE public.registry
    ADD CONSTRAINT registry_context_config CHECK (((kind <> ALL (ARRAY['memory'::text, 'source'::text])) OR (NOT ((metadata -> 'config'::text) ? 'schema_version'::text)) OR COALESCE(public.aidash_context_is_valid((metadata -> 'config'::text), kind), false))) NOT VALID;

--

ALTER TABLE ONLY public.registry_memory_role_refs
    ADD CONSTRAINT registry_memory_role_refs_pkey PRIMARY KEY (tenant, source_id, source_version, role, ordinal);

--

ALTER TABLE public.registry
    ADD CONSTRAINT registry_metadata_shape CHECK (((jsonb_typeof((metadata - ARRAY['installation'::text, 'binding_normalization'::text])) = 'object'::text) AND (jsonb_typeof(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'name'::text)) = 'object'::text) AND (((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'name'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - ARRAY['installation'::text, 'binding_normalization'::text]), 'strict $."name".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'description'::text)) = 'object'::text) AND (((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'description'::text) <> '{}'::jsonb) AND (NOT jsonb_path_exists((metadata - ARRAY['installation'::text, 'binding_normalization'::text]), 'strict $."description".*?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'config'::text)) = 'object'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'schema'::text), '{}'::jsonb)) = 'object'::text) AND public.jsonschema_is_valid((COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'schema'::text), '{}'::jsonb))::json) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND
CASE
    WHEN (jsonb_typeof((metadata - ARRAY['installation'::text, 'binding_normalization'::text])) = 'object'::text) THEN (((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) - ARRAY['id'::text, 'version'::text, 'kind'::text, 'name'::text, 'description'::text, 'capabilities'::text, 'tags'::text, 'languages'::text, 'skills'::text, 'schema'::text, 'config'::text]) = '{}'::jsonb)
    ELSE false
END AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'capabilities'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'capabilities'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'tags'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'tags'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'languages'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'languages'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((jsonb_typeof(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata - ARRAY['installation'::text, 'binding_normalization'::text]) -> 'skills'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true))) AND ((NOT (metadata ? 'installation'::text)) OR ((jsonb_typeof((metadata -> 'installation'::text)) = 'object'::text) AND (((metadata -> 'installation'::text) ->> 'contract'::text) = '1'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'tenant'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'installation'::text)) = 'string'::text) AND (jsonb_typeof(((metadata -> 'installation'::text) -> 'revision'::text)) = 'number'::text))))) NOT VALID;

--

ALTER TABLE ONLY public.registry
    ADD CONSTRAINT registry_pkey PRIMARY KEY (tenant, id, version);

--

ALTER TABLE ONLY public.registry_requests
    ADD CONSTRAINT registry_requests_pkey PRIMARY KEY (tenant, principal, key);

--

ALTER TABLE public.registry
    ADD CONSTRAINT registry_tool_config CHECK (((kind <> 'tool'::text) OR COALESCE(public.aidash_descriptor_is_valid((metadata -> 'config'::text)), false))) NOT VALID;

--

CREATE UNIQUE INDEX agent_draft_registered_version ON public.agent_draft_registrations USING btree (tenant, agent_id, version);

--

CREATE UNIQUE INDEX registry_id_version_kind_unique ON public.registry USING btree (tenant, id, version, kind);

--

ALTER TABLE ONLY public.agent_draft_registrations
    ADD CONSTRAINT agent_draft_registrations_draft_id_fkey FOREIGN KEY (tenant, draft_id) REFERENCES public.agent_drafts(tenant, id);

--

ALTER TABLE ONLY public.agent_draft_shares
    ADD CONSTRAINT agent_draft_shares_draft_id_fkey FOREIGN KEY (tenant, draft_id) REFERENCES public.agent_drafts(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.agent_incident_events
    ADD CONSTRAINT agent_incident_events_incident_id_fkey FOREIGN KEY (tenant, incident_id) REFERENCES public.agent_incidents(tenant, id);

--

ALTER TABLE ONLY public.agent_knowledge
    ADD CONSTRAINT agent_knowledge_agent_id_agent_version_fkey FOREIGN KEY (tenant, agent_id, agent_version) REFERENCES public.registry(tenant, id, version);

--

ALTER TABLE ONLY public.agent_knowledge_images
    ADD CONSTRAINT agent_knowledge_images_parent_fkey FOREIGN KEY (tenant, agent_id, agent_version) REFERENCES public.agent_knowledge(tenant, agent_id, agent_version) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.agent_test_sessions
    ADD CONSTRAINT agent_test_sessions_draft_id_fkey FOREIGN KEY (tenant, draft_id) REFERENCES public.agent_drafts(tenant, id);

--

ALTER TABLE ONLY public.registry_agent_model_refs
    ADD CONSTRAINT registry_agent_model_source FOREIGN KEY (tenant, agent_id, agent_version) REFERENCES public.registry(tenant, id, version) ON UPDATE CASCADE ON DELETE CASCADE;

--

ALTER TABLE ONLY public.registry_agent_model_refs
    ADD CONSTRAINT registry_agent_model_target FOREIGN KEY (tenant, model_id, model_version, model_kind) REFERENCES public.registry(tenant, id, version, kind) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.registry_agent_resource_refs
    ADD CONSTRAINT registry_agent_resource_source FOREIGN KEY (tenant, agent_id, agent_version) REFERENCES public.registry(tenant, id, version) ON UPDATE CASCADE ON DELETE CASCADE;

--

ALTER TABLE ONLY public.registry_agent_resource_refs
    ADD CONSTRAINT registry_agent_resource_target FOREIGN KEY (tenant, reference_id, reference_version, required_kind) REFERENCES public.registry(tenant, id, version, kind) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.registry_memory_role_refs
    ADD CONSTRAINT registry_memory_role_source FOREIGN KEY (tenant, source_id, source_version) REFERENCES public.registry(tenant, id, version) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.registry_memory_role_refs
    ADD CONSTRAINT registry_memory_role_target FOREIGN KEY (tenant, reference_id, reference_version, required_kind) REFERENCES public.registry(tenant, id, version, kind) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

SELECT pg_catalog.setval('public.agent_incident_events_id_seq', 1, false);
