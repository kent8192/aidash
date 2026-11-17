-- Rewriting stored levels is a data change that native migration operations
-- cannot express. A Tenant keeps its level; Standard is renamed. A Tenant that
-- had turned the Plan Connection exemption off above Standard could not use
-- Plan Connections, so it starts with them forbidden. Keyless endpoints start
-- allowed, as #962 decides for every Tenant.
SET LOCAL aidash.scope = 'deployment';
UPDATE public.tenant_data_protection
   SET forbid_plan_connections = NOT plan_connection_exempt AND level <> 'standard';
UPDATE public.tenant_data_protection_history
   SET forbid_plan_connections = NOT plan_connection_exempt AND level <> 'standard';
UPDATE public.tenant_data_protection SET level = 'no_restriction' WHERE level = 'standard';
UPDATE public.tenant_data_protection_history SET level = 'no_restriction' WHERE level = 'standard';
