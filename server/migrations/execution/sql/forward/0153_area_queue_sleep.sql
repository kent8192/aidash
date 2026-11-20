-- PostgreSQL function bodies and triggers cannot be expressed by typed
-- Reinhardt operations. An execution holds its Working Area's queue only
-- while it is awake (#1008, ADR 0140 amendment of 2026-11-20): a wait that
-- ends its Stint (every wait but failure delivery, the rule
-- aidash_stint_transition records Stints by) or a pause marks it asleep once
-- no worker holds it, and the executions that no awake one precedes any
-- longer are told they may run. Waking, it re-enters at the tail when it is
-- leased (execution::activation, `rejoin`).
CREATE FUNCTION public.aidash_area_queue_release(released uuid) RETURNS void
    LANGUAGE sql
    AS $$
    INSERT INTO run_activations (run_id, tenant, run_revision, reason)
    SELECT r.id, r.tenant, r.revision, 'ordering_release' FROM core_runs mine
      JOIN core_runs next ON next.area_id = mine.area_id AND next.generation = mine.generation
      JOIN task_executions r ON r.id = next.run_id
     WHERE mine.run_id = released AND next.sequence > mine.sequence
       AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
       AND NOT EXISTS (
           SELECT 1 FROM core_runs earlier JOIN task_executions holder ON holder.id = earlier.run_id
            WHERE earlier.area_id = mine.area_id AND earlier.generation = mine.generation
              AND earlier.sequence < next.sequence AND earlier.run_id <> released
              AND NOT earlier.asleep
              AND holder.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
       )
     ORDER BY next.sequence;
$$;
CREATE FUNCTION public.aidash_area_queue_sleep() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Only the step into sleep releases the queue: an update of an execution
    -- that already slept, unleased and with the same control, leaves an
    -- execution that woke and waits at the tail in its place.
    IF NEW.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
       AND NEW.lease_owner IS NULL
       AND (NEW.control = 'PAUSED'
            OR (NEW.phase = 'WAITING' AND COALESCE(NEW.pending #>> '{data,reason}', '') <> 'failure_delivery'))
       AND (OLD.lease_owner IS NOT NULL
            OR OLD.control IS DISTINCT FROM NEW.control
            OR NOT (OLD.control = 'PAUSED'
                    OR (OLD.phase = 'WAITING' AND COALESCE(OLD.pending #>> '{data,reason}', '') <> 'failure_delivery'))) THEN
        UPDATE core_runs SET asleep = true WHERE run_id = NEW.id AND NOT asleep;
        IF FOUND THEN
            PERFORM aidash_area_queue_release(NEW.id);
        END IF;
    END IF;
    RETURN NULL;
END;
$$;
CREATE TRIGGER aidash_area_queue_sleep AFTER UPDATE OF phase, control, pending, lease_owner ON public.task_executions FOR EACH ROW EXECUTE FUNCTION public.aidash_area_queue_sleep();
CREATE OR REPLACE FUNCTION public.aidash_activation_trigger() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_TABLE_NAME = 'task_executions' THEN
        IF TG_OP = 'UPDATE' THEN
            -- Heartbeats and lease acquisition are not new scheduling intent.
            IF NEW.phase IS NOT DISTINCT FROM OLD.phase
               AND NEW.control IS NOT DISTINCT FROM OLD.control
               AND (CASE WHEN jsonb_typeof(NEW.pending) = 'object' THEN NEW.pending #- '{recovery,lease_recoveries}' ELSE NEW.pending END) IS NOT DISTINCT FROM (CASE WHEN jsonb_typeof(OLD.pending) = 'object' THEN OLD.pending #- '{recovery,lease_recoveries}' ELSE OLD.pending END)
               AND NEW.step IS NOT DISTINCT FROM OLD.step
               AND NOT (OLD.lease_owner IS NOT NULL AND NEW.lease_owner IS NULL) THEN
                RETURN NEW;
            END IF;
        END IF;
        PERFORM aidash_request_activation(NEW.id, 'run_transition');
        IF NEW.phase IN ('COMPLETED', 'FAILED', 'CANCELLED') THEN
            -- Only the head's end releases admission (#1008: so does its sleep,
            -- in aidash_area_queue_sleep).
            PERFORM aidash_area_queue_release(NEW.id);
        END IF;
    ELSIF TG_TABLE_NAME = 'tasks' THEN
        IF NEW.status IS DISTINCT FROM OLD.status THEN
            INSERT INTO run_activations (run_id, tenant, run_revision, reason)
            SELECT r.id, r.tenant, r.revision, 'dependency_release' FROM task_dependencies d
              JOIN task_executions r ON r.task_id = d.task_id
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
        IF NEW.kind IN ('outbound', 'disclosure', 'tool_call') THEN
            INSERT INTO run_activations (run_id, tenant, run_revision, reason)
            SELECT id, tenant, revision, 'approval' FROM task_executions
             WHERE id = (NEW.data ->> 'run_id')::UUID
               AND pending -> 'data' ->> 'reason' = 'approval'
               AND pending -> 'data' -> 'approval' ->> 'source' = 'capability'
               AND pending -> 'data' -> 'approval' ->> 'approval_id' = NEW.id::TEXT;
        END IF;
    ELSIF TG_TABLE_NAME = 'core_runs' THEN
        PERFORM aidash_request_activation(NEW.run_id, 'core_admission');
    ELSIF TG_TABLE_NAME = 'core_areas' THEN
        IF NEW.state IS DISTINCT FROM OLD.state THEN
            INSERT INTO run_activations (run_id, tenant, run_revision, reason)
            SELECT r.id, r.tenant, r.revision, 'area_release' FROM task_executions r JOIN core_runs c ON c.run_id = r.id
             WHERE c.area_id = NEW.id AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED');
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
