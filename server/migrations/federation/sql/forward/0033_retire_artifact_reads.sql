SET LOCAL aidash.scope = 'deployment';
DELETE FROM "authorization_remote_grant_reads" WHERE "resource_kind" = 'artifact';
