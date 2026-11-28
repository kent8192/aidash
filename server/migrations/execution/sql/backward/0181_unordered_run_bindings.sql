SET LOCAL aidash.scope = 'deployment';
UPDATE "core_runs" SET "sequence" = (SELECT count(*) FROM "core_runs" AS "earlier" WHERE "earlier"."area_id" = "core_runs"."area_id" AND "earlier"."run_id" <= "core_runs"."run_id"), "generation" = COALESCE((SELECT "generation" FROM "core_areas" WHERE "core_areas"."id" = "core_runs"."area_id" AND "core_areas"."tenant" = "core_runs"."tenant"), 1);
UPDATE "core_areas" SET "next_sequence" = (SELECT count(*) + 1 FROM "core_runs" WHERE "core_runs"."area_id" = "core_areas"."id");
