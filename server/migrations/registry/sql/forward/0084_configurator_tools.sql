-- PostgreSQL validation function DDL has no typed Reinhardt operation.
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_is_valid_before_configurator_0084(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'configurator tool contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_is_valid_before_configurator_0084(');
 IF copied = original THEN RAISE EXCEPTION 'descriptor contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' IS DISTINCT FROM 'core.configurator@1' THEN RETURN public.aidash_descriptor_is_valid_before_configurator_0084(value); END IF;
 IF value->>'operation' IS NULL OR value->>'operation' NOT IN ('aidash_api',
   'aidash_api_catalog',
   'tenant_overview',
   'node_update',
   'member_invite',
   'member_set_groups',
   'registry_create',
   'catalog_approve',
   'provider_binding_set',
   'budget_set',
   'approval_mode_set',
   'approval_rule_set',
   'tenant_instructions_set',
   'data_protection_set',
   'tenant_secret_set',
   'stint_start',
   'marketplace_install',
   'mcp_server_add',
   'tool_add',
   'agent_plugin_add',
   'skill_add',
   'reference_upload',
   'duty_template_install',
   'agent_create',
   'generation_policy_set',
   'node_registration')
  OR value->>'default_alias' IS DISTINCT FROM value->>'operation' THEN RETURN false; END IF;
 RETURN public.aidash_descriptor_is_valid_before_configurator_0084(value || jsonb_build_object(
  'provider', 'core.duties@1',
  'operation', 'duty_create',
  'default_alias', 'duty_create'));
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;
