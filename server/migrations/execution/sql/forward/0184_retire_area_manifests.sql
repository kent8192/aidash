SET LOCAL aidash.scope = 'deployment';
DELETE FROM "core_blob_references" WHERE "holder_kind" = 'revision';
UPDATE "core_objects" SET "kind" = 'superseded_working' WHERE "kind" IN ('working', 'received');
