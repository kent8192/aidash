-- Refuse reversal while a Source or a Run snapshot holds more files than the old limit.
DO $$
BEGIN
 IF EXISTS (SELECT 1 FROM public.registry WHERE kind = 'source' AND jsonb_typeof(metadata #> '{config,source,references}') = 'array' AND jsonb_array_length(metadata #> '{config,source,references}') > 8) THEN
  RAISE EXCEPTION 'registry 0063_reference_set_limit cannot be reversed: a Source binds more than 8 Reference files' USING ERRCODE = '55000';
 END IF;
 IF to_regclass('public.runs') IS NOT NULL THEN
  IF EXISTS (SELECT 1 FROM public.runs WHERE jsonb_path_exists(context, 'lax $.binding_snapshot.bindings[*].definition.config.source.references.size() ? (@ > 8)')) THEN
   RAISE EXCEPTION 'registry 0063_reference_set_limit cannot be reversed: a Run pins more than 8 Reference files' USING ERRCODE = '55000';
  END IF;
 END IF;
END
$$;
-- Restore the exact body from registry/0045_context_sources.
CREATE OR REPLACE FUNCTION public.aidash_context_is_valid(value jsonb, kind text) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE source jsonb; adapter text;
BEGIN
 IF jsonb_typeof(value) <> 'object' OR value - ARRAY['schema_version','source','context_source']::text[] <> '{}'::jsonb
 OR value->'schema_version' IS DISTINCT FROM '1'::jsonb OR jsonb_typeof(value->'source') <> 'object' THEN RETURN false; END IF;
 source := value->'source'; adapter := source->>'adapter';
 IF COALESCE(value->'context_source','null'::jsonb) <> 'null'::jsonb AND NOT COALESCE(public.aidash_source_declaration_is_valid(value->'context_source', CASE adapter WHEN 'private_references' THEN 'references' WHEN 'reference_attachments' THEN 'references' WHEN 'skill_attachments' THEN 'skills' WHEN 'skill_roots' THEN 'skills' ELSE adapter END),false) THEN RETURN false; END IF;
 IF kind = 'memory' THEN RETURN adapter IN ('conversation_memory','semantic_memory') AND source - 'adapter' = '{}'::jsonb; END IF;
 IF kind <> 'source' THEN RETURN false; END IF;
 RETURN CASE adapter
 WHEN 'workspace_observation' THEN source - 'adapter' = '{}'::jsonb
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
