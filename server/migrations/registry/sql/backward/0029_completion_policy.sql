-- PostgreSQL function bodies have no typed migration operation. DDL only.
-- Restore the 0022_context_policy Agent Binding contract. Downgrading refuses
-- stored Completion Policy declarations rather than leaving rows that the
-- restored contract and pre-0028 binaries reject. Agent definitions are the
-- only declarations left: registry/0025_remove_legacy_installations dropped
-- packages and installations before this migration. It also refuses while any
-- Run, finished or not, stores Completion Gate progress or a Binding snapshot
-- pinning an Agent definition that declares a Completion Policy: pre-0028
-- binaries reject both when they read the Run, including its history. Such a
-- Run's Context Journal events are covered by its progress, which every
-- Completion Gate decision records.
DO $$
BEGIN
 IF EXISTS(SELECT 1 FROM registry WHERE kind = 'agent' AND metadata->'config' ? 'completion') THEN
  RAISE EXCEPTION 'registry 0029_completion_policy cannot be reversed: Agent definitions declare completion';
 END IF;
 -- PL/pgSQL plans a condition whole, so a query naming a missing public.runs
 -- fails even behind a false to_regclass test. The nested IF is planned only
 -- once the table is known to exist.
 IF to_regclass('public.runs') IS NOT NULL THEN
  IF EXISTS(SELECT 1 FROM public.runs
    WHERE context ? 'completion'
    OR jsonb_path_exists(context, 'lax $.binding_snapshot.definitions[*].definition ? (@.kind == "agent" && exists(@.config.completion))')
    OR jsonb_path_exists(context, 'lax $.binding_snapshot.foreign_agents[*].definitions[*].definition ? (@.kind == "agent" && exists(@.config.completion))')) THEN
   RAISE EXCEPTION 'registry 0029_completion_policy cannot be reversed: Runs store Completion Gate progress or pin Agent definitions declaring completion';
  END IF;
 END IF;
END
$$;
CREATE OR REPLACE FUNCTION public.aidash_agent_bindings_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE item jsonb; removals jsonb; edges jsonb; step_count numeric; parallelism numeric; context_policy jsonb; summary jsonb; restriction jsonb; candidate jsonb; policy jsonb; deferred boolean; budget text; bound numeric; removable text[]; support text[];
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
 OR value - ARRAY['schema_version','model','instructions','bindings','remove_default','cluster','max_steps','projection_version','prompt_cache','exposure','tool_parallelism','context_policy']::text[] <> '{}'::jsonb
 OR NOT COALESCE(value->'projection_version','null'::jsonb) IN ('null'::jsonb,'"legacy"'::jsonb,'"ordered"'::jsonb,'"native"'::jsonb)
 OR NOT COALESCE(value->'prompt_cache','null'::jsonb) IN ('null'::jsonb,'"off"'::jsonb,'"explicit"'::jsonb)
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb
 OR NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'model' || '{"registry_node":"aidash://contract"}'::jsonb),false)
 OR jsonb_typeof(COALESCE(value->'instructions','""'::jsonb)) <> 'string' THEN RETURN false; END IF;
 IF COALESCE(value->'cluster','null'::jsonb) <> 'null'::jsonb
 AND NOT COALESCE(public.aidash_qualified_ref_is_valid(value->'cluster' || '{"registry_node":"aidash://contract"}'::jsonb),false) THEN RETURN false; END IF;
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
   OR NOT COALESCE(public.aidash_qualified_ref_is_valid(summary->'model' || '{"registry_node":"aidash://contract"}'::jsonb),false)
   OR jsonb_typeof(COALESCE(summary->'max_tokens','4096'::jsonb)) <> 'number' OR NOT COALESCE(summary->>'max_tokens','4096') ~ '^[0-9]+$'
   OR jsonb_typeof(COALESCE(summary->'call_budget','8'::jsonb)) <> 'number' OR NOT COALESCE(summary->>'call_budget','8') ~ '^[0-9]+$' THEN RETURN false; END IF;
   IF COALESCE(summary->>'max_tokens','4096')::numeric NOT BETWEEN 256 AND 4096
   OR COALESCE(summary->>'call_budget','8')::numeric NOT BETWEEN 1 AND 64 THEN RETURN false; END IF;
  END IF;
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
  removable := ARRAY['workspace_observe','workspace_wait','skill_asset_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','memory_mutate','memory_recall','memory_reflect'];
  support := ARRAY['skill_asset_read'];
 ELSE
  removable := ARRAY['workspace_observe','workspace_wait','skill_list','skill_load','skill_read','file_search','file_read','task_create','task_delegate','agent_discover','artifact_publish','workspace_message','memory_mutate','memory_recall','memory_reflect'];
  support := ARRAY['skill_list','skill_load','skill_read'];
 END IF;
 FOR item IN SELECT jsonb_array_elements(edges) LOOP
  IF jsonb_typeof(item) <> 'object' OR item - ARRAY['kind','target','alias','narrow','members','exposure']::text[] <> '{}'::jsonb
  OR COALESCE(item->'exposure','null'::jsonb) <> 'null'::jsonb
  AND NOT (deferred AND item->>'kind' IN ('tool','bundle','skill') AND COALESCE(item->>'exposure' IN ('eager','deferred'),false))
  OR NOT COALESCE(item->>'kind' IN ('tool','bundle','skill','memory','source'),false)
  OR NOT COALESCE(public.aidash_qualified_ref_is_valid(item->'target'),false)
  OR jsonb_typeof(COALESCE(item->'narrow','{}'::jsonb)) <> 'object'
  OR COALESCE(item->'narrow','{}'::jsonb) - ARRAY['allowed_hosts','scope','limits','concurrency']::text[] <> '{}'::jsonb THEN RETURN false; END IF;
  restriction := COALESCE(item->'narrow','{}'::jsonb);
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
 OR EXISTS(SELECT 1 FROM jsonb_array_elements(edges) e WHERE e->>'kind' IN ('skill','source'));
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;
