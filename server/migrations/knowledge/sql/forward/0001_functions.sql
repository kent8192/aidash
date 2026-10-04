SET LOCAL search_path = public, pg_catalog;

SET LOCAL check_function_bodies = false;
CREATE FUNCTION public.guard_semantic_index_input_limit() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
DECLARE
    max_bytes_text text;
BEGIN
    max_bytes_text := NEW.spec->>'max_input_bytes';
    -- Let the row CHECK report malformed specs; this trigger only enforces the
    -- cross-table invariant once the limit has the expected numeric shape.
    IF jsonb_typeof(NEW.spec->'max_input_bytes') <> 'number'
       OR max_bytes_text !~ '^(0|[1-9][0-9]*)$' THEN
        RETURN NEW;
    END IF;
    IF EXISTS (
        SELECT 1 FROM semantic_entries e
        WHERE e.workspace_id = NEW.workspace_id
          AND e.source->>'kind' = 'memory'
          AND octet_length(e.source->>'text') > max_bytes_text::numeric
    ) THEN
        RAISE EXCEPTION 'semantic index max_input_bytes is below existing active memory text'
            USING ERRCODE = '23514', CONSTRAINT = 'semantic_index_input_bytes';
    END IF;
    RETURN NEW;
END $_$;
CREATE FUNCTION public.guard_semantic_memory_entry_bytes() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
DECLARE
    max_bytes_text text;
BEGIN
    IF NEW.source->>'kind' <> 'memory' THEN
        RETURN NEW;
    END IF;

    -- Lock the workspace's index row so a concurrent index-limit reduction
    -- cannot race an entry write that exceeds the new limit.
    SELECT spec->>'max_input_bytes' INTO max_bytes_text
    FROM semantic_indexes
    WHERE workspace_id = NEW.workspace_id
    FOR UPDATE;
    IF NOT FOUND OR max_bytes_text !~ '^(0|[1-9][0-9]*)$' THEN
        RAISE EXCEPTION 'active semantic memory requires a valid index input limit'
            USING ERRCODE = '23514', CONSTRAINT = 'semantic_memory_input_bytes';
    END IF;
    IF octet_length(NEW.source->>'text') > max_bytes_text::numeric THEN
        RAISE EXCEPTION 'semantic memory text exceeds the index max_input_bytes'
            USING ERRCODE = '23514', CONSTRAINT = 'semantic_memory_input_bytes';
    END IF;
    RETURN NEW;
END $_$;
