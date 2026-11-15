SET LOCAL aidash.scope = 'deployment';
DELETE FROM authorization_catalog WHERE entry_id IN ('core.npm', 'core.pypi');
