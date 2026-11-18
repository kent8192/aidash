SET LOCAL aidash.scope = 'deployment';
INSERT INTO "tenant_data_protection" ("tenant", "level", "model_call_zdr", "plan_connection_exempt", "updated_at", "updated_by") SELECT "name", 'standard', TRUE, TRUE, CURRENT_TIMESTAMP, 'migration' FROM "tenants" WHERE NOT EXISTS (SELECT 1 FROM "tenant_data_protection" WHERE "tenant_data_protection"."tenant" = "tenants"."name");
UPDATE "tenant_data_protection" SET "forbid_plan_connections" = NOT "plan_connection_exempt" AND "level" <> 'standard';
UPDATE "tenant_data_protection_history" SET "forbid_plan_connections" = NOT "plan_connection_exempt" AND "level" <> 'standard';
UPDATE "tenant_data_protection" SET "level" = 'no_restriction' WHERE "level" = 'standard';
UPDATE "tenant_data_protection_history" SET "level" = 'no_restriction' WHERE "level" = 'standard';
