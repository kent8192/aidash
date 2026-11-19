SET LOCAL aidash.scope = 'deployment';
UPDATE "duties" SET "halt" = 'requester_left' WHERE "halt" = 'registrant_left';
UPDATE "duties" SET "halt" = 'paused' WHERE "halt" = 'interval_too_short';
