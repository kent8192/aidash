SET LOCAL aidash.scope = 'deployment';
UPDATE "core_runs" SET "scope_subject" = (SELECT "owner" FROM "core_areas" WHERE "core_areas"."id" = "core_runs"."area_id" AND "core_areas"."tenant" = "core_runs"."tenant");
