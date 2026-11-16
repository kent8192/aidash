-- Research is no longer a category (#987, ADR 0138): of the research contract
-- providers only page reading's fetch remains. PostgreSQL validation functions
-- require SQL DDL; this wraps the current contract and keeps all its rules.
SET LOCAL aidash.scope = 'deployment';
DO $$
DECLARE original text; copied text;
BEGIN
 IF to_regprocedure('public.aidash_descriptor_before_research_0069(jsonb)') IS NOT NULL THEN
  RAISE EXCEPTION 'Research descriptor contract backup already exists';
 END IF;
 original := pg_get_functiondef('public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
 copied := replace(original, 'FUNCTION public.aidash_descriptor_is_valid(', 'FUNCTION public.aidash_descriptor_before_research_0069(');
 IF copied = original THEN RAISE EXCEPTION 'Descriptor contract header missing'; END IF;
 EXECUTE copied;
END $$;

CREATE OR REPLACE FUNCTION public.aidash_descriptor_is_valid(value jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
BEGIN
 IF value->>'provider' LIKE 'research.%'
 AND (value->>'provider', value->>'operation', value->>'tier') IS DISTINCT FROM ('research.page@1', 'fetch', 'research') THEN
  RETURN false;
 END IF;
 RETURN public.aidash_descriptor_before_research_0069(value);
EXCEPTION WHEN OTHERS THEN RETURN false;
END $$;

-- Drafts keep no Binding to a removed definition. Registered Agent versions
-- are immutable and keep theirs; the definition rows stay so they resolve.
UPDATE agent_drafts
SET entry = jsonb_set(entry, '{config,bindings}', (
    SELECT COALESCE(jsonb_agg(item.binding ORDER BY item.ordinal), '[]'::jsonb)
    FROM jsonb_array_elements(entry #> '{config,bindings}') WITH ORDINALITY AS item(binding, ordinal)
    WHERE COALESCE(item.binding #>> '{target,id}', '') NOT IN (
        'aidash.research.brave.search',
        'aidash.research.page.find', 'aidash.research.page.view',
        'aidash.research.fixture.search', 'aidash.research.fixture.fetch',
        'aidash.research.fixture.find', 'aidash.research.fixture.view',
        'aidash.bundle.research_brave', 'aidash.bundle.research_fixture')
))
WHERE jsonb_typeof(entry #> '{config,bindings}') = 'array'
  AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(entry #> '{config,bindings}') AS found(binding)
    WHERE found.binding #>> '{target,id}' IN (
        'aidash.research.brave.search',
        'aidash.research.page.find', 'aidash.research.page.view',
        'aidash.research.fixture.search', 'aidash.research.fixture.fetch',
        'aidash.research.fixture.find', 'aidash.research.fixture.view',
        'aidash.bundle.research_brave', 'aidash.bundle.research_fixture')
  );
