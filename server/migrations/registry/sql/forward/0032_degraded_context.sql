-- Degraded context (#234, ADR 0059): a `workspace_retrieval` source may declare
-- `on_unavailable: pause | degrade`; no other adapter may. PostgreSQL function
-- bodies have no typed Reinhardt operation. DDL only; no application data is
-- modified, and the NOT VALID constraints that call this function keep their
-- existing rows.
CREATE OR REPLACE FUNCTION public.aidash_context_is_valid(value jsonb, kind text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE source jsonb; adapter text;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['schema_version','source']::text[] <> '{}'::jsonb
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb OR jsonb_typeof(value->'source') <> 'object' THEN RETURN false; END IF;
 source := value->'source'; adapter := source->>'adapter';
 IF kind = 'memory' THEN RETURN adapter IN ('conversation_memory','semantic_memory') AND source - 'adapter' = '{}'::jsonb; END IF;
 IF kind <> 'source' THEN RETURN false; END IF;
 RETURN CASE adapter
 WHEN 'workspace_retrieval' THEN source - ARRAY['adapter','on_unavailable']::text[] = '{}'::jsonb
  AND (NOT source ? 'on_unavailable' OR (jsonb_typeof(source->'on_unavailable') = 'string' AND source->>'on_unavailable' IN ('pause','degrade')))
 WHEN 'private_references' THEN source - ARRAY['adapter','digest']::text[] = '{}'::jsonb AND source->>'digest' ~ '^[a-fA-F0-9]{64}$'
 WHEN 'reference_attachments' THEN source - ARRAY['adapter','references']::text[] = '{}'::jsonb AND jsonb_typeof(source->'references') = 'array' AND jsonb_array_length(source->'references') BETWEEN 1 AND 8
 WHEN 'skill_attachments' THEN source - ARRAY['adapter','attachments']::text[] = '{}'::jsonb AND jsonb_typeof(source->'attachments') = 'array' AND jsonb_array_length(source->'attachments') BETWEEN 1 AND 16
 WHEN 'skill_roots' THEN source - ARRAY['adapter','roots']::text[] = '{}'::jsonb AND jsonb_typeof(source->'roots') = 'array' AND jsonb_array_length(source->'roots') BETWEEN 1 AND 8 AND NOT jsonb_path_exists(source->'roots','strict $[*] ? (@.type() != "string")')
 ELSE false END;
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;
