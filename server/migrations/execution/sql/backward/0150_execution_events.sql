SET LOCAL aidash.scope = 'deployment';
UPDATE "events" SET "kind" = 'run.' || substr("kind", 11) WHERE "kind" LIKE 'execution.%';
