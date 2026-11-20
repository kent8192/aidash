SET LOCAL aidash.scope = 'deployment';
UPDATE "events" SET "kind" = 'execution.' || substr("kind", 5) WHERE "kind" LIKE 'run.%';
