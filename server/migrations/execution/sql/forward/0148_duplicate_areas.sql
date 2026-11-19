SET LOCAL aidash.scope = 'deployment';
DELETE FROM "core_areas" WHERE EXISTS (SELECT 1 FROM "core_areas" AS "newer" WHERE newer.tenant = core_areas.tenant AND newer.home_node = core_areas.home_node AND newer.workspace_id = core_areas.workspace_id AND newer.agent_id = core_areas.agent_id AND newer.owner = core_areas.owner AND newer.id > core_areas.id);
