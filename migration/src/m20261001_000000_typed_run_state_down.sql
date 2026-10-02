-- PostgreSQL procedural function/trigger DDL exceptions: SeaQuery has no builders.
CREATE FUNCTION guard_waiting_human_request() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.pending ? 'human_request_id' THEN
        IF jsonb_typeof(NEW.pending->'human_request_id') <> 'string'
           OR NEW.pending->>'human_request_id' !~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' THEN
            RAISE EXCEPTION 'pending human request id must be a string naming a request for this run'
                USING ERRCODE = '23514', CONSTRAINT = 'runs_waiting_request';
        END IF;
    END IF;

    IF NEW.phase = 'WAITING'
       AND NOT COALESCE(aidash_valid_pending_timestamp(NEW.pending->'wake_at'), false)
       AND NOT (NEW.pending ? 'human_request_id') THEN
        RAISE EXCEPTION 'waiting run requires a valid wake_at or a request belonging to the run'
            USING ERRCODE = '23514', CONSTRAINT = 'runs_waiting_request';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER runs_waiting_request_guard
    BEFORE INSERT OR UPDATE OF id, phase, pending ON runs
    FOR EACH ROW EXECUTE FUNCTION guard_waiting_human_request();
CREATE OR REPLACE FUNCTION legacy_run_input_bridge() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
    target_run uuid;
    current_run record;
    existing_content text;
BEGIN
    IF NEW.idempotency_key ~ ('^human:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(NEW.idempotency_key, ':', 2)::uuid;
    ELSIF NEW.idempotency_key LIKE 'subject-human:%'
        AND right(NEW.idempotency_key, 74) ~ ('^:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(right(NEW.idempotency_key, 74), ':', 2)::uuid;
    ELSE
        RETURN NEW;
    END IF;

    SELECT r.phase, r.control, r.pending INTO current_run
    FROM runs AS r WHERE r.id = target_run AND r.workspace_id = NEW.workspace_id FOR UPDATE;
    IF NOT FOUND THEN
        RETURN NEW;
    END IF;

    SELECT content INTO existing_content FROM run_inputs
    WHERE run_id = target_run AND idempotency_key = NEW.idempotency_key;
    IF FOUND THEN
        IF existing_content <> NEW.content THEN
            RAISE EXCEPTION 'run message idempotency key reused';
        END IF;
        UPDATE run_inputs SET message_id = NEW.id
        WHERE run_id = target_run AND idempotency_key = NEW.idempotency_key
            AND (message_id IS NULL OR message_id = NEW.id);
        IF NOT FOUND THEN
            RAISE EXCEPTION 'run input message binding changed';
        END IF;
        RETURN NEW;
    END IF;

    IF current_run.pending->>'finalizing' = 'true'
        OR current_run.pending ? 'terminal_transition'
        OR current_run.control = 'CANCELLED'
        OR current_run.phase IN ('COMPLETED', 'FAILED', 'CANCELLED') THEN
        RAISE EXCEPTION 'run message arrived after finalization';
    END IF;
    INSERT INTO run_inputs (run_id, sender, content, idempotency_key, message_id)
    VALUES (target_run, NEW.sender, NEW.content, NEW.idempotency_key, NEW.id);
    RETURN NEW;
END;
$$;
