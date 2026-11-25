SET LOCAL aidash.scope = 'deployment';
WITH updated_bundles AS (
UPDATE "authorization_bundles" SET "document" = (replace(replace(document::text, '"duty.create"', '"schedule.create"'), '"duty.administer"', '"schedule.administer"'))::jsonb, "revision" = "revision" + 1, "updated_at" = CURRENT_TIMESTAMP WHERE (document::text LIKE '%"duty.create"%' OR document::text LIKE '%"duty.administer"%') RETURNING "tenant", "revision", "document"
)
INSERT INTO "authorization_revisions" ("tenant", "revision", "document", "actor") SELECT "tenant", "revision", "document", 'migration:identity/0098_duty_member_policy' FROM "updated_bundles";
