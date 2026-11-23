SET LOCAL aidash.scope = 'deployment';
INSERT INTO "authorization_revisions" ("tenant", "revision", "document", "actor") SELECT "tenant", "revision", "document", 'migration:identity/0114_policy_revision_backfill' FROM "authorization_bundles" ON CONFLICT ("tenant", "revision") DO NOTHING;
