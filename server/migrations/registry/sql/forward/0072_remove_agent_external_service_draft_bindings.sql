-- A one-time data rewrite kept as SQL (#738). Reinhardt Query at the pinned
-- revision cannot build this statement: a FROM source is only a table or a
-- subquery (no set-returning function such as jsonb_array_elements, no WITH
-- ORDINALITY and no column alias list), and a function call takes plain
-- arguments only, so jsonb_agg cannot carry ORDER BY to keep Binding order.
SET LOCAL aidash.scope = 'deployment';
UPDATE agent_drafts
SET entry = jsonb_set(entry, '{config,bindings}', (
    SELECT COALESCE(jsonb_agg(item.binding ORDER BY item.ordinal), '[]'::jsonb)
    FROM jsonb_array_elements(entry #> '{config,bindings}') WITH ORDINALITY AS item(binding, ordinal)
    WHERE COALESCE(item.binding ->> 'kind', '') <> 'external_service'
))
WHERE jsonb_typeof(entry #> '{config,bindings}') = 'array'
  AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(entry #> '{config,bindings}') AS found(binding)
    WHERE found.binding ->> 'kind' = 'external_service'
  );
