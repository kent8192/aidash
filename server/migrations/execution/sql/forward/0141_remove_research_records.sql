SET LOCAL aidash.scope = 'deployment';
DELETE FROM "approval_observations" WHERE "approval_type" = 'capability' AND ("approval" IN (SELECT "id" FROM "core_records" WHERE "kind" = 'disclosure') OR "approval" IN (SELECT "approval" FROM "approval_observation_population" WHERE "kind" IN ('web_search', 'page_reading')));
DELETE FROM "approval_observation_population" WHERE "kind" IN ('web_search', 'page_reading');
DELETE FROM "core_records" WHERE "kind" = 'disclosure';
