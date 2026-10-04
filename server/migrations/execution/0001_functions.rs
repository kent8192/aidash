// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0001_functions", "execution")
        .add_dependency("operations", "0000_environment")
        .add_operation(Operation::RunSQL {
            sql: r#"SET LOCAL check_function_bodies = false;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.aidash_activation_trigger() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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
$$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.aidash_model_response_is_valid(input_value jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE
    AS $_$
DECLARE
    tool_call jsonb;
    token_value text;
BEGIN
    IF jsonb_typeof(input_value) IS DISTINCT FROM 'object'
       OR jsonb_typeof(input_value->'text') IS DISTINCT FROM 'string'
       OR jsonb_typeof(input_value->'tool_calls') IS DISTINCT FROM 'array'
       OR jsonb_typeof(input_value->'input_tokens') IS DISTINCT FROM 'number'
       OR jsonb_typeof(input_value->'output_tokens') IS DISTINCT FROM 'number' THEN
        RETURN false;
    END IF;
    IF input_value ? 'usage_complete' AND jsonb_typeof(input_value->'usage_complete') <> 'boolean' THEN
        RETURN false;
    END IF;
    FOREACH token_value IN ARRAY ARRAY[
        input_value->>'input_tokens',
        input_value->>'output_tokens'
    ] LOOP
        IF token_value !~ '^(0|[1-9][0-9]*)$'
           OR token_value::numeric > 18446744073709551615 THEN
            RETURN false;
        END IF;
    END LOOP;
    FOR tool_call IN SELECT value FROM jsonb_array_elements(input_value->'tool_calls') LOOP
        IF jsonb_typeof(tool_call) IS DISTINCT FROM 'object'
           OR jsonb_typeof(tool_call->'id') IS DISTINCT FROM 'string'
           OR jsonb_typeof(tool_call->'name') IS DISTINCT FROM 'string'
           OR NOT (tool_call ? 'arguments') THEN
            RETURN false;
        END IF;
    END LOOP;
    RETURN true;
EXCEPTION WHEN OTHERS THEN
    RETURN false;
END
$_$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.aidash_request_activation(target uuid, cause text) RETURNS void
    LANGUAGE sql
    AS $$
    INSERT INTO run_activations (run_id, run_revision, reason)
    SELECT id, revision, cause FROM runs WHERE id = target;
$$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.aidash_valid_pending_timestamp(input_value jsonb) RETURNS boolean
    LANGUAGE plpgsql STABLE
    AS $$
DECLARE parsed timestamptz;
BEGIN
    IF jsonb_typeof(input_value) <> 'string' THEN RETURN false; END IF;
    parsed := (input_value #>> '{}')::timestamptz;
    RETURN isfinite(parsed);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.gate_legacy_federated_run_message() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
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
$_$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.gate_legacy_run_message() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
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
$_$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.gate_legacy_run_output() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
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
$_$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.gate_legacy_run_worker() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- A worker that already held a lease when this migration ran must not
    -- commit an answer against inputs that were accepted beforehand.
    IF OLD.lease_owner IS NOT NULL AND NOT OLD.ledger_worker_ready
        AND EXISTS (SELECT 1 FROM run_inputs WHERE run_id = OLD.id)
        AND current_setting('aidash.input_ledger_worker', true) IS DISTINCT FROM 'true'
        AND (NEW.phase IS DISTINCT FROM OLD.phase
            OR NEW.pending IS DISTINCT FROM OLD.pending
            OR NEW.context IS DISTINCT FROM OLD.context
            OR NEW.step IS DISTINCT FROM OLD.step
            OR NEW.observed_input_seq IS DISTINCT FROM OLD.observed_input_seq
            OR NEW.revision IS DISTINCT FROM OLD.revision) THEN
        RAISE EXCEPTION 'run input ledger requires an upgraded worker';
    END IF;
    IF NEW.lease_owner IS NOT NULL
        AND (NEW.lease_owner IS DISTINCT FROM OLD.lease_owner
            OR NEW.lease_until > OLD.lease_until)
        AND (OLD.ledger_worker_ready OR EXISTS
            (SELECT 1 FROM run_inputs WHERE run_id = OLD.id))
        AND current_setting('aidash.input_ledger_worker', true) IS DISTINCT FROM 'true' THEN
        RAISE EXCEPTION 'run input ledger requires an upgraded worker';
    END IF;
    RETURN NEW;
END;
$$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.legacy_run_input_bridge() RETURNS trigger
    LANGUAGE plpgsql
    AS $_$
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
$_$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.marketplace_catalog_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE owner_tenant text;
BEGIN
 SELECT metadata->'installation'->>'tenant' INTO owner_tenant FROM registry WHERE id=NEW.entry_id AND version=NEW.entry_version;
 IF owner_tenant IS NOT NULL AND (owner_tenant <> NEW.tenant OR current_setting('aidash.marketplace_writer',true) IS DISTINCT FROM '1') THEN
  RAISE EXCEPTION 'unsupported Marketplace catalog writer';
 END IF;
 RETURN NEW;
END $$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.marketplace_immutable() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN RAISE EXCEPTION 'Marketplace content is immutable'; END $$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.marketplace_overlay_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF EXISTS (SELECT 1 FROM registry WHERE id=NEW.id AND version=NEW.version AND metadata ? 'installation') THEN RAISE EXCEPTION 'Marketplace projections cannot have legacy overlays'; END IF;
 RETURN NEW;
END $$;"#.to_string(),
            // Refuse irreversible baseline rollback before the native ledger changes.
            reverse_sql: Some(r#"DO $aidash_baseline$
BEGIN
    RAISE EXCEPTION 'Aidash frozen baseline is forward-only; restore a backup to roll back';
END
$aidash_baseline$;"#.to_string()),
        })
        .atomic(true)
        .database_only(true)
}
