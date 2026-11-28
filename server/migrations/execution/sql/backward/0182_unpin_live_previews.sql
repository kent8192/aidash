SET LOCAL aidash.scope = 'deployment';
UPDATE "core_live_previews" SET "area_id" = (SELECT "area_id" FROM "core_runs" WHERE "core_runs"."run_id" = "core_live_previews"."run_id" AND "core_runs"."tenant" = "core_live_previews"."tenant");
UPDATE "core_live_previews" SET "revision" = (SELECT "revision" FROM "core_areas" WHERE "core_areas"."id" = "core_live_previews"."area_id" AND "core_areas"."tenant" = "core_live_previews"."tenant");
