SET LOCAL aidash.scope = 'deployment';
UPDATE "authorization_bundles" SET "document" = (replace(replace(document::text, '"duty.create"', '"schedule.create"'), '"duty.administer"', '"schedule.administer"'))::jsonb, "revision" = "revision" + 1, "updated_at" = CURRENT_TIMESTAMP WHERE (document::text LIKE '%"duty.create"%' OR document::text LIKE '%"duty.administer"%');
