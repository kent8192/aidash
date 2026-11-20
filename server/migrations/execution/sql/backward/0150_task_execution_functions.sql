-- PostgreSQL function bodies cannot be expressed by typed Reinhardt operations.
-- These bodies name the runs table again (#1009). The reversal restores them
-- before the table is renamed back, so the LANGUAGE sql body of
-- aidash_request_activation is not checked until then.
SET LOCAL check_function_bodies = false;
CREATE OR REPLACE FUNCTION public.aidash_activation_trigger() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_TABLE_NAME = 'runs' THEN
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
            -- Only the head's completion releases admission. Notify one successor,
            -- skipping terminal gaps while preserving paused predecessors.
            INSERT INTO run_activations (run_id, tenant, run_revision, reason)
            SELECT r.id, r.tenant, r.revision, 'ordering_release' FROM core_runs mine
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
            INSERT INTO run_activations (run_id, tenant, run_revision, reason)
            SELECT r.id, r.tenant, r.revision, 'dependency_release' FROM task_dependencies d
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
        IF NEW.kind IN ('outbound', 'disclosure', 'tool_call') THEN
            INSERT INTO run_activations (run_id, tenant, run_revision, reason)
            SELECT id, tenant, revision, 'approval' FROM runs
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
            SELECT r.id, r.tenant, r.revision, 'area_release' FROM runs r JOIN core_runs c ON c.run_id = r.id
             WHERE c.area_id = NEW.id AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED');
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION public.aidash_request_activation(target uuid, cause text) RETURNS void
    LANGUAGE sql
    AS $$
    INSERT INTO run_activations (run_id, tenant, run_revision, reason)
    SELECT id, tenant, revision, cause FROM runs WHERE id = target;
$$;
CREATE OR REPLACE FUNCTION public.aidash_thread_task_activation()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
DECLARE target uuid;
BEGIN
    IF NEW.status IS NOT DISTINCT FROM OLD.status
       OR NEW.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED', 'BLOCKED') THEN
        RETURN NEW;
    END IF;
    FOR target IN
	    SELECT o.source_run_id FROM authorization_task_origins o
	      JOIN runs r ON r.id = o.source_run_id AND r.tenant = o.tenant
	     WHERE o.task_id = NEW.id AND o.tenant = NEW.tenant
	       AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
        UNION
	    SELECT o.run_id FROM authorization_run_outputs o
	      JOIN runs r ON r.id = o.run_id AND r.tenant = o.tenant
	     WHERE o.resource_kind = 'delegated_task' AND o.resource_id = NEW.id
	       AND o.tenant = NEW.tenant AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
        ORDER BY 1
    LOOP
        -- A dedicated Run-keyed advisory lock orders producer commits without
        -- taking the Run row or reversing cancellation's row-lock order.
        PERFORM pg_advisory_xact_lock(hashtextextended('aidash:thread-wait:' || target::text, 71500644));
        PERFORM aidash_request_activation(target, 'thread_task:' || NEW.id::text || ':' || NEW.status);
    END LOOP;
    RETURN NEW;
END;
$function$;
CREATE OR REPLACE FUNCTION public.gate_legacy_run_message()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
    target_run uuid;
BEGIN
    IF NEW.idempotency_key IS NULL THEN
        IF EXISTS (SELECT 1 FROM runs WHERE thread_id = NEW.thread_id
            AND phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')) THEN
            RAISE EXCEPTION 'unkeyed messages require upgraded run admission while a run is active';
        END IF;
        RETURN NEW;
    END IF;

    IF NEW.idempotency_key ~ ('^human:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(NEW.idempotency_key, ':', 2)::uuid;
    ELSIF NEW.idempotency_key LIKE 'subject-human:%'
        AND right(NEW.idempotency_key, 74) ~ ('^:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(right(NEW.idempotency_key, 74), ':', 2)::uuid;
    ELSE
        RETURN NEW;
    END IF;

    IF EXISTS (SELECT 1 FROM runs WHERE id = target_run AND thread_id = NEW.thread_id)
        AND NOT EXISTS (SELECT 1 FROM run_inputs
            WHERE run_id = target_run AND idempotency_key = NEW.idempotency_key) THEN
        RAISE EXCEPTION 'run-directed messages require upgraded ledger admission';
    END IF;
    RETURN NEW;
END;
$function$;
CREATE OR REPLACE FUNCTION public.gate_legacy_run_output()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
    target_run uuid;
    target_task uuid;
    peer_key_parts text[];
BEGIN
    IF NEW.idempotency_key ~ ('^' || uuid_pattern || ':[0-9]+:output$') THEN
        target_run := split_part(NEW.idempotency_key, ':', 1)::uuid;
    ELSIF NEW.idempotency_key ~ ('^.*:' || uuid_pattern || ':' || uuid_pattern || ':[0-9]+:output$') THEN
        peer_key_parts := regexp_match(
            NEW.idempotency_key,
            '^.*:(' || uuid_pattern || '):(' || uuid_pattern || '):[0-9]+:output$'
        );
        target_task := peer_key_parts[1]::uuid;
        target_run := peer_key_parts[2]::uuid;
    END IF;

	IF target_task IS NOT NULL THEN
		-- Peer-prefixed output is written on the home node, where the executor's
		-- run row does not exist. Serialize against the task reservation and reject
		-- an old home API write while its remote input fence is still active.
		PERFORM id FROM tasks
		WHERE id = target_task AND thread_id = NEW.thread_id
		FOR UPDATE;
		IF FOUND
			AND current_setting('aidash.input_ledger_worker', true) IS DISTINCT FROM 'true'
			AND EXISTS (
				SELECT 1 FROM remote_run_message_fences
				WHERE task_id = target_task
				  AND run_id = target_run
				  AND NOT consumed
				  AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
			) THEN
			RAISE EXCEPTION 'run input ledger requires fenced response publication';
		END IF;
	END IF;

    IF target_run IS NOT NULL THEN
        PERFORM id FROM runs
        WHERE id = target_run
          AND thread_id = NEW.thread_id
          AND (target_task IS NULL OR task_id = target_task)
        FOR UPDATE;
        IF FOUND
            AND current_setting('aidash.input_ledger_worker', true) IS DISTINCT FROM 'true'
            AND EXISTS (SELECT 1 FROM run_inputs WHERE run_id = target_run) THEN
            RAISE EXCEPTION 'run input ledger requires fenced response publication';
        END IF;
    END IF;
    RETURN NEW;
END;
$function$;
CREATE OR REPLACE FUNCTION public.gate_remote_task_terminal() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.status IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED')
        AND NEW.status IS DISTINCT FROM OLD.status THEN
        PERFORM id FROM runs WHERE task_id = OLD.id FOR UPDATE;
        IF EXISTS (
                SELECT 1 FROM remote_run_message_fences
                WHERE task_id = OLD.id
                  AND NOT consumed
                  AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
            ) OR EXISTS (
                SELECT 1 FROM runs AS r
                WHERE r.task_id = OLD.id
                  AND r.lease_owner IS NOT NULL
                  AND NOT r.ledger_worker_ready
                  AND EXISTS (
                      SELECT 1 FROM run_inputs AS i
                      WHERE i.run_id = r.id AND i.seq > r.observed_input_seq
                  )
            ) THEN
            RAISE EXCEPTION USING ERRCODE = 'A3301',
                MESSAGE = 'run messages await inference';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION public.legacy_run_input_bridge()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
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
    FROM runs AS r WHERE r.id = target_run AND r.thread_id = NEW.thread_id FOR UPDATE;
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
    INSERT INTO run_inputs (run_id, tenant, sender, content, idempotency_key, message_id)
    VALUES (target_run, NEW.tenant, NEW.sender, NEW.content, NEW.idempotency_key, NEW.id);
    RETURN NEW;
END;
$function$;
