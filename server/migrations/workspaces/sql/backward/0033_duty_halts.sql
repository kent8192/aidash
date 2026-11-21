SET LOCAL aidash.scope = 'deployment';
UPDATE "duties" SET "halt" = 'registrant_left' WHERE "halt" = 'requester_left';
