SET LOCAL aidash.scope = 'deployment';
UPDATE agent_drafts
SET entry = jsonb_set(entry, '{config,bindings}', (
    SELECT COALESCE(jsonb_agg(item.binding ORDER BY item.ordinal), '[]'::jsonb)
    FROM jsonb_array_elements(entry #> '{config,bindings}') WITH ORDINALITY AS item(binding, ordinal)
    WHERE COALESCE(item.binding #>> '{target,id}', '') NOT IN ('core.npm', 'core.pypi')
))
WHERE jsonb_typeof(entry #> '{config,bindings}') = 'array'
  AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(entry #> '{config,bindings}') AS found(binding)
    WHERE found.binding #>> '{target,id}' IN ('core.npm', 'core.pypi')
  );
