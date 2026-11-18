SET LOCAL aidash.scope = 'deployment';
UPDATE "tenant_data_protection" SET "plan_connection_exempt" = NOT "forbid_plan_connections", "model_call_zdr" = TRUE;
UPDATE "tenant_data_protection_history" SET "plan_connection_exempt" = NOT "forbid_plan_connections", "model_call_zdr" = TRUE;
UPDATE "tenant_data_protection" SET "level" = 'standard' WHERE "level" = 'no_restriction';
UPDATE "tenant_data_protection_history" SET "level" = 'standard' WHERE "level" = 'no_restriction';
UPDATE "tenant_data_protection" SET "level" = 'zero_data_retention' WHERE "level" = 'safety_retention';
UPDATE "tenant_data_protection_history" SET "level" = 'zero_data_retention' WHERE "level" = 'safety_retention';
