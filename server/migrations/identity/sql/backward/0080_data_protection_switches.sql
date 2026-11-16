-- Restores the former levels: Safety Retention did not exist, so it becomes
-- Zero Data Retention, the nearest level that is not weaker.
SET LOCAL aidash.scope = 'deployment';
UPDATE public.tenant_data_protection
   SET plan_connection_exempt = NOT forbid_plan_connections, model_call_zdr = true;
UPDATE public.tenant_data_protection_history
   SET plan_connection_exempt = NOT forbid_plan_connections, model_call_zdr = true;
UPDATE public.tenant_data_protection SET level = 'standard' WHERE level = 'no_restriction';
UPDATE public.tenant_data_protection_history SET level = 'standard' WHERE level = 'no_restriction';
UPDATE public.tenant_data_protection SET level = 'zero_data_retention' WHERE level = 'safety_retention';
UPDATE public.tenant_data_protection_history SET level = 'zero_data_retention' WHERE level = 'safety_retention';
