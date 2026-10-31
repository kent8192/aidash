SELECT set_config('aidash.scope', 'deployment', TRUE), set_config('aidash.tenant', '', TRUE);
UPDATE "public"."runs" SET "created_at" = "updated_at";
