SET LOCAL aidash.scope = 'deployment';
DELETE FROM "authorization_remote_commands" WHERE "request_key" LIKE 'run_message_complete:%';
