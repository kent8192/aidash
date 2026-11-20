SET LOCAL aidash.scope = 'deployment';
DELETE FROM "budget_notices" WHERE "sent_at" IS NULL;
