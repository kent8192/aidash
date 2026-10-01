-- SeaQuery cannot alter CHECK constraints.
-- Keep scalar lifecycle/counter constraints; executable JSON is validated by
-- the row codec so damaged/unsupported rows remain stoppable and repairable.
ALTER TABLE runs DROP CONSTRAINT runs_counters;
ALTER TABLE runs ADD CONSTRAINT runs_counters CHECK (step >= 0 AND revision >= 0 AND revision < 9223372036854775807);
-- Waiting shape/deadline validation belongs to the strict Rust codec. The
-- SeaQuery-built composite foreign key remains the request binding guard.
-- PostgreSQL trigger/function DDL exceptions: SeaQuery has no such builders.
DROP TRIGGER runs_waiting_request_guard ON runs;
DROP FUNCTION guard_waiting_human_request();
-- PostgreSQL function DDL exception: SeaQuery has no procedural builder.
CREATE OR REPLACE FUNCTION aidash_activation_trigger() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'runs' THEN
        IF TG_OP = 'UPDATE' THEN
            -- Heartbeats and lease acquisition are not new scheduling intent.
            IF NEW.phase IS NOT DISTINCT FROM OLD.phase
               AND NEW.control IS NOT DISTINCT FROM OLD.control
               AND (CASE WHEN jsonb_typeof(NEW.pending) = 'object' THEN NEW.pending #- '{recovery,lease_recovered}' ELSE NEW.pending END) IS NOT DISTINCT FROM (CASE WHEN jsonb_typeof(OLD.pending) = 'object' THEN OLD.pending #- '{recovery,lease_recovered}' ELSE OLD.pending END)
               AND NEW.step IS NOT DISTINCT FROM OLD.step
               AND NOT (OLD.lease_owner IS NOT NULL AND NEW.lease_owner IS NULL) THEN
                RETURN NEW;
            END IF;
        END IF;
        PERFORM aidash_request_activation(NEW.id, 'run_transition');
        IF NEW.phase IN ('COMPLETED', 'FAILED', 'CANCELLED') THEN
            -- Only the head's completion releases admission. Notify one successor,
            -- skipping terminal gaps while preserving paused predecessors.
            INSERT INTO run_activations (run_id, run_revision, reason)
            SELECT r.id, r.revision, 'ordering_release' FROM core_runs mine
              JOIN core_runs next ON next.area_id = mine.area_id AND next.generation = mine.generation
              JOIN runs r ON r.id = next.run_id
             WHERE mine.run_id = NEW.id AND next.sequence > mine.sequence
               AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
               AND NOT EXISTS (
                   SELECT 1 FROM core_runs earlier JOIN runs pending ON pending.id = earlier.run_id
                    WHERE earlier.area_id = mine.area_id AND earlier.generation = mine.generation
                      AND earlier.sequence < mine.sequence
                      AND pending.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
               )
             ORDER BY next.sequence LIMIT 1;
        END IF;
    ELSIF TG_TABLE_NAME = 'tasks' THEN
        IF NEW.status IS DISTINCT FROM OLD.status THEN
            INSERT INTO run_activations (run_id, run_revision, reason)
            SELECT r.id, r.revision, 'dependency_release' FROM task_dependencies d
              JOIN runs r ON r.task_id = d.task_id
             WHERE d.dependency_id = NEW.id
               AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED');
        END IF;
    ELSIF TG_TABLE_NAME = 'run_inputs' THEN
        PERFORM aidash_request_activation(NEW.run_id, 'input');
    ELSIF TG_TABLE_NAME = 'human_requests' THEN
        IF NEW.response IS DISTINCT FROM OLD.response THEN
            PERFORM aidash_request_activation(NEW.run_id, 'human_response');
        END IF;
    ELSIF TG_TABLE_NAME = 'core_records' THEN
        IF NEW.kind = 'approval' THEN
            INSERT INTO run_activations (run_id, run_revision, reason)
            SELECT id, revision, 'approval' FROM runs
             WHERE id = (NEW.data ->> 'run_id')::UUID
               AND pending -> 'data' ->> 'reason' = 'core_approval' AND pending -> 'data' ->> 'approval_id' = NEW.id::TEXT;
        END IF;
    ELSIF TG_TABLE_NAME = 'core_runs' THEN
        PERFORM aidash_request_activation(NEW.run_id, 'core_admission');
    ELSIF TG_TABLE_NAME = 'core_areas' THEN
        IF NEW.state IS DISTINCT FROM OLD.state THEN
            INSERT INTO run_activations (run_id, run_revision, reason)
            SELECT r.id, r.revision, 'area_release' FROM runs r JOIN core_runs c ON c.run_id = r.id
             WHERE c.area_id = NEW.id AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED');
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

-- Input admission remains fenced across the current finalization format.
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

    IF current_run.pending->'data'->>'finalizing' = 'true'
        OR current_run.pending->'data'->>'reason' = 'failure_delivery'
        OR current_run.control = 'CANCELLED'
        OR current_run.phase IN ('COMPLETED', 'FAILED', 'CANCELLED') THEN
        RAISE EXCEPTION 'run message arrived after finalization';
    END IF;
    INSERT INTO run_inputs (run_id, sender, content, idempotency_key, message_id)
    VALUES (target_run, NEW.sender, NEW.content, NEW.idempotency_key, NEW.id);
    RETURN NEW;
END;
$$;
