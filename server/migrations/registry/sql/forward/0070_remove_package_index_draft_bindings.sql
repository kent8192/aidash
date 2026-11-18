-- Raw SQL: removing elements from the JSON Binding arrays needs jsonb_array_elements ...
-- WITH ORDINALITY and jsonb_agg, which Reinhardt Query cannot express.
SET LOCAL aidash.scope = 'deployment';
-- Drafts.
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
-- Registered Agent versions, Marketplace copies included (as registry/0054 did). Both the
-- Agent's own Bindings and the normalized copy registration saved are rewritten, so the
-- normalization still matches the definition; the reference tables follow through their
-- triggers.
ALTER TABLE public.registry DISABLE TRIGGER marketplace_registry_fence;
UPDATE public.registry
SET metadata = jsonb_set(metadata, '{config,bindings}', (
    SELECT COALESCE(jsonb_agg(item.binding ORDER BY item.ordinal), '[]'::jsonb)
    FROM jsonb_array_elements(metadata #> '{config,bindings}') WITH ORDINALITY AS item(binding, ordinal)
    WHERE COALESCE(item.binding #>> '{target,id}', '') NOT IN ('core.npm', 'core.pypi')
))
WHERE kind = 'agent'
  AND jsonb_typeof(metadata #> '{config,bindings}') = 'array'
  AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(metadata #> '{config,bindings}') AS found(binding)
    WHERE found.binding #>> '{target,id}' IN ('core.npm', 'core.pypi')
  );
UPDATE public.registry
SET metadata = jsonb_set(metadata, '{binding_normalization,bindings}', (
    SELECT COALESCE(jsonb_agg(item.normalized ORDER BY item.ordinal), '[]'::jsonb)
    FROM jsonb_array_elements(metadata #> '{binding_normalization,bindings}') WITH ORDINALITY AS item(normalized, ordinal)
    WHERE COALESCE(item.normalized #>> '{binding,target,id}', '') NOT IN ('core.npm', 'core.pypi')
))
WHERE kind = 'agent'
  AND jsonb_typeof(metadata #> '{binding_normalization,bindings}') = 'array'
  AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(metadata #> '{binding_normalization,bindings}') AS found(normalized)
    WHERE found.normalized #>> '{binding,target,id}' IN ('core.npm', 'core.pypi')
  );
ALTER TABLE public.registry ENABLE TRIGGER marketplace_registry_fence;
-- Registration requests replay against the registered version they created.
UPDATE public.registry_requests
SET request = jsonb_set(request, '{config,bindings}', (
    SELECT COALESCE(jsonb_agg(item.binding ORDER BY item.ordinal), '[]'::jsonb)
    FROM jsonb_array_elements(request #> '{config,bindings}') WITH ORDINALITY AS item(binding, ordinal)
    WHERE COALESCE(item.binding #>> '{target,id}', '') NOT IN ('core.npm', 'core.pypi')
))
WHERE request->>'kind' = 'agent'
  AND jsonb_typeof(request #> '{config,bindings}') = 'array'
  AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(request #> '{config,bindings}') AS found(binding)
    WHERE found.binding #>> '{target,id}' IN ('core.npm', 'core.pypi')
  );
