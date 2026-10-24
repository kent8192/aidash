-- The registry/0023_research_providers descriptor contract plus the research `view` operation of research.page@1 and research.fixture@1 (#312).
CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
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
  WHEN 'workspace_wait' THEN 'core.workspace@1' WHEN 'workspace_message' THEN 'core.workspace@1'
  WHEN 'human_request' THEN 'core.human@1'
  WHEN 'task_create' THEN 'core.tasks@1' WHEN 'task_delegate' THEN 'core.tasks@1'
  WHEN 'agent_discover' THEN 'core.tasks@1' WHEN 'task_assign' THEN 'core.tasks@1'
  WHEN 'skill_list' THEN 'core.skills@1' WHEN 'skill_load' THEN 'core.skills@1' WHEN 'skill_read' THEN 'core.skills@1'
  WHEN 'file_search' THEN 'core.files@1' WHEN 'file_read' THEN 'core.files@1' WHEN 'apply_patch' THEN 'core.files@1'
  WHEN 'artifact_publish' THEN 'core.artifacts@1' WHEN 'memory_mutate' THEN 'core.memory@1' WHEN 'memory_recall' THEN 'core.memory@1' WHEN 'memory_reflect' THEN 'core.memory@1'
  WHEN 'shell' THEN 'core.sandbox@1' WHEN 'shell_poll' THEN 'core.sandbox@1' WHEN 'shell_cancel' THEN 'core.sandbox@1'
  WHEN 'code_interpreter' THEN 'core.sandbox@1' WHEN 'python_install' THEN 'core.sandbox@1'
  WHEN 'python_poll' THEN 'core.sandbox@1' WHEN 'python_cancel' THEN 'core.sandbox@1'
  WHEN 'outbound_get' THEN 'core.egress@1' WHEN 'file_share' THEN 'core.sharing@1' END;
 IF expected_provider IS NOT NULL THEN
  expected_tier := CASE WHEN value->>'operation' IN ('shell','shell_poll','shell_cancel','code_interpreter','python_install','python_poll','python_cancel','outbound_get','apply_patch','file_share') THEN 'sandboxed' ELSE 'builtin' END;
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
$$;
