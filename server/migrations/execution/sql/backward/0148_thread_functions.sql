-- PostgreSQL function bodies cannot be expressed by typed Reinhardt operations.
-- These bodies name workspace_id again (#1008).
CREATE OR REPLACE FUNCTION public.gate_legacy_federated_run_message()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
BEGIN
    IF NEW.idempotency_key ~ ('^.+:' || uuid_pattern || ':human:' || uuid_pattern || ':' || uuid_pattern || '$')
        OR NEW.idempotency_key ~ ('^.+:' || uuid_pattern || ':subject-human:.+:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        IF current_setting('aidash.run_message_delivery', true) IS DISTINCT FROM 'true' THEN
            IF NOT EXISTS (SELECT 1 FROM messages AS existing
                WHERE existing.idempotency_key = NEW.idempotency_key
                    AND existing.workspace_id = NEW.workspace_id
                    AND existing.sender = NEW.sender
                    AND existing.content = NEW.content) THEN
                RAISE EXCEPTION 'federated run messages require upgraded ledger delivery';
            END IF;
        END IF;
    END IF;
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
        IF EXISTS (SELECT 1 FROM runs WHERE workspace_id = NEW.workspace_id
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

    IF EXISTS (SELECT 1 FROM runs WHERE id = target_run AND workspace_id = NEW.workspace_id)
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
		WHERE id = target_task AND workspace_id = NEW.workspace_id
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
          AND workspace_id = NEW.workspace_id
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
    INSERT INTO run_inputs (run_id, tenant, sender, content, idempotency_key, message_id)
    VALUES (target_run, NEW.tenant, NEW.sender, NEW.content, NEW.idempotency_key, NEW.id);
    RETURN NEW;
END;
$function$;
CREATE OR REPLACE FUNCTION public.aidash_delegated_task_activation()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
DECLARE task_status text;
BEGIN
    IF NEW.resource_kind <> 'delegated_task' THEN
        RETURN NEW;
    END IF;
    SELECT status INTO task_status FROM tasks
     WHERE id = NEW.resource_id AND tenant = NEW.tenant;
    IF task_status IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED', 'BLOCKED') THEN
        PERFORM pg_advisory_xact_lock(hashtextextended('aidash:workspace-wait:' || NEW.run_id::text, 71500644));
        PERFORM aidash_request_activation(NEW.run_id, 'workspace_task:' || NEW.resource_id::text || ':' || task_status);
    END IF;
    RETURN NEW;
END;
$function$;
DROP TRIGGER aidash_thread_task_activation ON public.tasks;
DROP FUNCTION public.aidash_thread_task_activation();
CREATE OR REPLACE FUNCTION public.aidash_workspace_task_activation()
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
        PERFORM pg_advisory_xact_lock(hashtextextended('aidash:workspace-wait:' || target::text, 71500644));
        PERFORM aidash_request_activation(target, 'workspace_task:' || NEW.id::text || ':' || NEW.status);
    END LOOP;
    RETURN NEW;
END;
$function$;
CREATE TRIGGER aidash_workspace_task_activation AFTER UPDATE OF status ON public.tasks FOR EACH ROW EXECUTE FUNCTION public.aidash_workspace_task_activation();
