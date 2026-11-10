SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE FUNCTION public.aidash_activation_trigger() RETURNS trigger
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

--

CREATE FUNCTION public.aidash_delegated_task_activation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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
$$;

--

CREATE FUNCTION public.aidash_model_response_is_valid(input_value jsonb) RETURNS boolean
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
$_$;

--

CREATE FUNCTION public.aidash_operation_finished_activation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.state IN ('prepared', 'submitted', 'running', 'cancelling')
       AND NEW.state NOT IN ('prepared', 'submitted', 'running', 'cancelling') THEN
        PERFORM aidash_request_activation(NEW.run_id, 'operation_finished');
    END IF;
    RETURN NEW;
END;
$$;

--

CREATE FUNCTION public.aidash_release_message_retention() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    DELETE FROM public.core_retention_holds
     WHERE holder_kind = 'message'
       AND holder_id = OLD.id::text
       AND tenant = OLD.tenant;
    RETURN OLD;
END;
$$;

--

CREATE FUNCTION public.aidash_request_activation(target uuid, cause text) RETURNS void
    LANGUAGE sql
    AS $$
    INSERT INTO run_activations (run_id, tenant, run_revision, reason)
    SELECT id, tenant, revision, cause FROM runs WHERE id = target;
$$;

--

CREATE FUNCTION public.aidash_run_control_provenance() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.control IS DISTINCT FROM OLD.control
        AND NEW.control_revision = OLD.control_revision THEN
        NEW.control_revision := OLD.control_revision + 1;
        IF NEW.control_source = OLD.control_source THEN
            NEW.control_source := 'system';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

--

CREATE FUNCTION public.aidash_valid_pending_timestamp(input_value jsonb) RETURNS boolean
    LANGUAGE plpgsql STABLE
    AS $$
DECLARE parsed timestamptz;
BEGIN
    IF jsonb_typeof(input_value) <> 'string' THEN RETURN false; END IF;
    parsed := (input_value #>> '{}')::timestamptz;
    RETURN isfinite(parsed);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$$;

--

CREATE FUNCTION public.aidash_workspace_task_activation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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
$$;

--

CREATE FUNCTION public.gate_legacy_federated_run_message() RETURNS trigger
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
$_$;

--

CREATE FUNCTION public.gate_legacy_run_message() RETURNS trigger
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
$_$;

--

CREATE FUNCTION public.gate_legacy_run_output() RETURNS trigger
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
$_$;

--

CREATE FUNCTION public.gate_legacy_run_worker() RETURNS trigger
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
$$;

--

CREATE FUNCTION public.legacy_run_input_bridge() RETURNS trigger
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
    INSERT INTO run_inputs (run_id, tenant, sender, content, idempotency_key, message_id)
    VALUES (target_run, NEW.tenant, NEW.sender, NEW.content, NEW.idempotency_key, NEW.id);
    RETURN NEW;
END;
$_$;

--

CREATE FUNCTION public.marketplace_catalog_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE owner_tenant text;
BEGIN
 SELECT metadata->'installation'->>'tenant' INTO owner_tenant FROM registry WHERE tenant=NEW.tenant AND id=NEW.entry_id AND version=NEW.entry_version;
 IF owner_tenant IS NOT NULL AND (owner_tenant <> NEW.tenant OR current_setting('aidash.marketplace_writer',true) IS DISTINCT FROM '1') THEN
  RAISE EXCEPTION 'unsupported Marketplace catalog writer';
 END IF;
 RETURN NEW;
END $$;

--

CREATE FUNCTION public.marketplace_immutable() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- Only a Tenant purge deletes it, with the Tenant's installation (ADR 0061).
    IF TG_OP = 'DELETE' AND public.aidash_tenant_purge() THEN RETURN OLD; END IF;
    RAISE EXCEPTION 'Marketplace content is immutable';
END $$;

--

CREATE FUNCTION public.marketplace_overlay_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF EXISTS (SELECT 1 FROM registry WHERE tenant=NEW.tenant AND id=NEW.id AND version=NEW.version AND metadata ? 'installation') THEN RAISE EXCEPTION 'Marketplace projections cannot have legacy overlays'; END IF;
 RETURN NEW;
END $$;

--

CREATE TABLE public.activation_quarantine (
    digest text NOT NULL,
    reason text NOT NULL,
    stream_sequence bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

--

CREATE TABLE public.approval_observation_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    observation uuid NOT NULL,
    decision uuid NOT NULL,
    record jsonb NOT NULL,
    status text NOT NULL,
    answers jsonb,
    provider_report jsonb,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.approval_observation_attempts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.approval_observation_population (
    approval uuid NOT NULL,
    tenant text NOT NULL,
    approval_type text NOT NULL,
    kind text NOT NULL,
    observer_unavailable jsonb,
    run_id uuid,
    settlement text NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.approval_observation_population FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.approval_observations (
    id uuid NOT NULL,
    tenant text NOT NULL,
    approval uuid NOT NULL,
    approval_type text NOT NULL,
    run_id uuid,
    workspace_id uuid NOT NULL,
    credential_id uuid NOT NULL,
    subject text NOT NULL,
    frame jsonb NOT NULL,
    evidence jsonb,
    settlement text NOT NULL,
    worker uuid,
    lease_revision bigint NOT NULL,
    leased_until timestamp with time zone,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.approval_observations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.approval_observer_settings (
    tenant text NOT NULL,
    revision bigint NOT NULL,
    settings jsonb NOT NULL,
    updated_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.approval_observer_settings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.context_compaction_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    stage text NOT NULL,
    policy_version text NOT NULL,
    provider text NOT NULL,
    source_from_seq bigint NOT NULL,
    source_through_seq bigint NOT NULL,
    base_revision bigint NOT NULL,
    observed_input_seq bigint NOT NULL,
    before_tokens bigint NOT NULL,
    outcome text,
    reason text,
    candidate_digest text,
    after_tokens bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    settled_at timestamp with time zone,
    provider_dispatched boolean,
    CONSTRAINT context_compaction_attempt_outcome CHECK (((outcome IS NULL) OR (outcome = ANY (ARRAY['adopted'::text, 'insufficient'::text, 'unavailable'::text, 'invalid'::text, 'unauthorized'::text, 'abandoned'::text])))),
    CONSTRAINT context_compaction_attempt_range CHECK (((source_from_seq >= 0) AND (source_through_seq >= source_from_seq) AND (before_tokens >= 0) AND ((after_tokens IS NULL) OR (after_tokens >= 0)))),
    CONSTRAINT context_compaction_attempt_settled CHECK (((outcome IS NULL) = (settled_at IS NULL))),
    CONSTRAINT context_compaction_attempt_stage CHECK ((stage = ANY (ARRAY['prune'::text, 'summary'::text])))
);

ALTER TABLE ONLY public.context_compaction_attempts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_areas (
    id uuid NOT NULL,
    tenant text NOT NULL,
    home_node text NOT NULL,
    agent_id text NOT NULL,
    owner text NOT NULL,
    workspace_id uuid NOT NULL,
    thread_id uuid NOT NULL,
    generation bigint DEFAULT 1 NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    epoch bigint DEFAULT 1 NOT NULL,
    next_sequence bigint DEFAULT 1 NOT NULL,
    state text DEFAULT 'active'::text NOT NULL,
    constraints jsonb DEFAULT '[]'::jsonb NOT NULL
);

ALTER TABLE ONLY public.core_areas FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_blob_references (
    id uuid NOT NULL,
    tenant text NOT NULL,
    digest text NOT NULL,
    holder_kind text NOT NULL,
    holder_id text NOT NULL,
    object_id uuid,
    area_id uuid,
    revision bigint,
    record_id uuid,
    CONSTRAINT core_blob_references_receipt_check CHECK (((holder_kind <> 'receipt'::text) OR ((record_id IS NOT NULL) AND (object_id IS NOT NULL)))),
    CONSTRAINT core_blob_references_revision_check CHECK (((holder_kind <> 'revision'::text) OR ((area_id IS NOT NULL) AND (revision IS NOT NULL) AND (object_id IS NOT NULL))))
);

ALTER TABLE ONLY public.core_blob_references FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_blob_uploads (
    id uuid NOT NULL,
    tenant text NOT NULL,
    size bigint NOT NULL,
    digest text,
    CONSTRAINT core_blob_uploads_size_check CHECK ((size >= 0))
);

ALTER TABLE ONLY public.core_blob_uploads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_blobs (
    tenant text NOT NULL,
    digest text NOT NULL,
    size bigint NOT NULL,
    CONSTRAINT core_blobs_digest_check CHECK ((digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT core_blobs_size_check CHECK ((size >= 0))
);

ALTER TABLE ONLY public.core_blobs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_live_previews (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    area_id uuid NOT NULL,
    revision bigint NOT NULL,
    port integer NOT NULL,
    state text NOT NULL,
    stop_reason text,
    idle_seconds bigint NOT NULL,
    absolute_seconds bigint NOT NULL,
    started_at timestamp with time zone NOT NULL,
    last_request_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    pod_host text,
    termination_confirmed boolean DEFAULT true NOT NULL,
    egress_expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT core_live_previews_limits CHECK (((idle_seconds >= 60) AND (absolute_seconds >= idle_seconds))),
    CONSTRAINT core_live_previews_port CHECK (((port >= 1024) AND (port <= 65535))),
    CONSTRAINT core_live_previews_state CHECK ((((state = ANY (ARRAY['starting'::text, 'running'::text, 'stale'::text])) AND (stop_reason IS NULL)) OR ((state = 'stopped'::text) AND (stop_reason IS NOT NULL))))
);

ALTER TABLE ONLY public.core_live_previews FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_manifest_revisions (
    tenant text NOT NULL,
    area_id uuid NOT NULL,
    revision bigint NOT NULL,
    manifest jsonb NOT NULL
);

ALTER TABLE ONLY public.core_manifest_revisions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_objects (
    id uuid NOT NULL,
    tenant text NOT NULL,
    area_id uuid,
    kind text NOT NULL,
    digest text NOT NULL,
    size bigint NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.core_objects FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_operations (
    id uuid NOT NULL,
    area_id uuid NOT NULL,
    run_id uuid NOT NULL,
    credential_id uuid NOT NULL,
    tenant text NOT NULL,
    principal text NOT NULL,
    request_key text NOT NULL,
    digest text NOT NULL,
    kind text NOT NULL,
    state text NOT NULL,
    epoch bigint NOT NULL,
    generation bigint NOT NULL,
    revision bigint NOT NULL,
    policy_revision bigint NOT NULL,
    subjects jsonb NOT NULL,
    input jsonb NOT NULL,
    result jsonb NOT NULL,
    runner_instance text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.core_operations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_preview_limits (
    tenant text NOT NULL,
    idle_seconds bigint NOT NULL,
    absolute_seconds bigint NOT NULL,
    revision bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.core_preview_limits FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_preview_sessions (
    id uuid NOT NULL,
    tenant text NOT NULL,
    preview_id uuid NOT NULL,
    subject text NOT NULL,
    credential_id uuid NOT NULL,
    origin text NOT NULL,
    digest text NOT NULL,
    served_bytes bigint DEFAULT 0 NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_seen_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    peer_sender text,
    peer_receiver text,
    peer_subject text,
    peer_run_id uuid
);

ALTER TABLE ONLY public.core_preview_sessions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_preview_tickets (
    id uuid NOT NULL,
    tenant text NOT NULL,
    preview_id uuid NOT NULL,
    subject text NOT NULL,
    credential_id uuid NOT NULL,
    origin text NOT NULL,
    digest text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    peer_sender text,
    peer_receiver text,
    peer_subject text,
    peer_run_id uuid
);

ALTER TABLE ONLY public.core_preview_tickets FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_previews (
    id uuid NOT NULL,
    tenant text NOT NULL,
    kind text NOT NULL,
    area_id uuid,
    file_id uuid,
    live_preview_id uuid,
    label text NOT NULL,
    created_by text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT core_previews_viewable CHECK ((((kind = 'file'::text) AND (area_id IS NOT NULL) AND (file_id IS NOT NULL) AND (live_preview_id IS NULL)) OR ((kind = 'live'::text) AND (live_preview_id IS NOT NULL) AND (area_id IS NULL) AND (file_id IS NULL))))
);

ALTER TABLE ONLY public.core_previews FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_quotas (
    tenant text NOT NULL,
    used_bytes bigint DEFAULT 0 NOT NULL,
    reserved_bytes bigint DEFAULT 0 NOT NULL,
    CONSTRAINT core_quotas_reserved_nonnegative CHECK ((reserved_bytes >= 0))
);

ALTER TABLE ONLY public.core_quotas FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_records (
    id uuid NOT NULL,
    tenant text NOT NULL,
    owner text NOT NULL,
    area_id uuid,
    kind text NOT NULL,
    state text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    data jsonb NOT NULL,
    expires_at timestamp with time zone
);

ALTER TABLE ONLY public.core_records FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_requests (
    tenant text NOT NULL,
    principal text NOT NULL,
    key uuid NOT NULL,
    digest text NOT NULL,
    result jsonb NOT NULL
);

ALTER TABLE ONLY public.core_requests FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_retention_files (
    id uuid NOT NULL,
    hold_id uuid NOT NULL,
    file_id uuid NOT NULL,
    tenant text NOT NULL,
    path text NOT NULL,
    size bigint NOT NULL,
    media_type text NOT NULL,
    CONSTRAINT retention_file_size CHECK ((size >= 0))
);

ALTER TABLE ONLY public.core_retention_files FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_retention_holds (
    id uuid NOT NULL,
    tenant text NOT NULL,
    area_id uuid NOT NULL,
    revision bigint NOT NULL,
    holder_kind text NOT NULL,
    holder_id text NOT NULL,
    peer text,
    expires_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT retention_federation_holder CHECK ((((holder_kind = 'federation'::text) = (expires_at IS NOT NULL)) AND ((holder_kind = 'federation'::text) = (peer IS NOT NULL)))),
    CONSTRAINT retention_holder_kind CHECK ((holder_kind = ANY (ARRAY['message'::text, 'live_preview'::text, 'federation'::text])))
);

ALTER TABLE ONLY public.core_retention_holds FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_runs (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    area_id uuid NOT NULL,
    sequence bigint NOT NULL,
    initialized boolean DEFAULT false NOT NULL,
    generation bigint DEFAULT 1 NOT NULL
);

ALTER TABLE ONLY public.core_runs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.core_task_sessions (
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    thread_id uuid NOT NULL
);

ALTER TABLE ONLY public.core_task_sessions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.decision_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    decision_id uuid NOT NULL,
    run_id uuid NOT NULL,
    node text NOT NULL,
    step integer NOT NULL,
    input_revision bigint NOT NULL,
    input_digest text NOT NULL,
    worker uuid NOT NULL,
    run_revision bigint NOT NULL,
    decider jsonb NOT NULL,
    binding_restrictions jsonb NOT NULL,
    request_digest text NOT NULL,
    state_digest text NOT NULL,
    questions jsonb NOT NULL,
    mode text NOT NULL,
    owner_receipts jsonb NOT NULL,
    status text NOT NULL,
    answers jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    question_digest text NOT NULL,
    cycle integer NOT NULL,
    retry integer NOT NULL,
    route integer NOT NULL,
    provider_report jsonb,
    CONSTRAINT decision_attempt_shape CHECK (((mode = ANY (ARRAY['shadow'::text, 'enforce'::text])) AND (step >= 0) AND (cycle >= 0) AND (retry >= 0) AND (route >= 0) AND (input_revision >= 0) AND (run_revision >= 0) AND (request_digest ~~ 'sha256:%'::text) AND (state_digest ~~ 'sha256:%'::text) AND (input_digest ~~ 'sha256:%'::text) AND (question_digest ~~ 'sha256:%'::text) AND (jsonb_typeof(questions) = 'array'::text) AND (jsonb_array_length(questions) > 0) AND (jsonb_typeof(owner_receipts) = 'object'::text) AND (owner_receipts <> '{}'::jsonb) AND ((answers IS NULL) OR (jsonb_typeof(answers) = 'object'::text)))),
    CONSTRAINT decision_attempt_status CHECK (((status = ANY (ARRAY['pending'::text, 'answered'::text, 'failed'::text, 'uncertain'::text, 'not_dispatched'::text])) AND ((status = 'pending'::text) = (finished_at IS NULL)) AND ((status = 'answered'::text) = (answers IS NOT NULL))))
);

ALTER TABLE ONLY public.decision_attempts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.decision_evidence (
    id uuid NOT NULL,
    tenant text NOT NULL,
    decision_id uuid NOT NULL,
    run_id uuid NOT NULL,
    outcome text NOT NULL,
    reason text NOT NULL,
    evidence jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT decision_evidence_outcome CHECK (((outcome = ANY (ARRAY['applied'::text, 'staged'::text, 'shadow'::text, 'rejected'::text, 'allowed'::text, 'blocked'::text])) AND (reason = ANY (ARRAY['fits'::text, 'insufficient'::text, 'forbidden'::text, 'provider_failure'::text, 'authority_failure'::text, 'journal_failure'::text, 'invalid_answers'::text, 'incomplete_answers'::text, 'no_candidates'::text, 'clear'::text, 'data_exfiltration'::text, 'destructive'::text, 'beyond_request'::text, 'injected'::text, 'boundary'::text, 'unavailable'::text])) AND (jsonb_typeof(evidence) = 'object'::text)))
);

ALTER TABLE ONLY public.decision_evidence FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.decision_outcomes (
    id uuid NOT NULL,
    tenant text NOT NULL,
    decision_id uuid NOT NULL,
    run_id uuid NOT NULL,
    next_inference text,
    observed_status text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT decision_outcome_bounds CHECK ((((length(observed_status) >= 1) AND (length(observed_status) <= 256)) AND ((next_inference IS NULL) OR ((length(next_inference) >= 1) AND (length(next_inference) <= 256)))))
);

ALTER TABLE ONLY public.decision_outcomes FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.decision_states (
    id uuid NOT NULL,
    tenant text NOT NULL,
    evidence_id uuid NOT NULL,
    run_id uuid NOT NULL,
    digest text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    state jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT decision_state_digest CHECK ((digest ~~ 'sha256:%'::text))
);

ALTER TABLE ONLY public.decision_states FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.egress_budgets (
    id uuid NOT NULL,
    tenant text NOT NULL,
    operation_id uuid NOT NULL,
    service text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    deadline_at timestamp with time zone,
    sent_bytes bigint NOT NULL,
    received_bytes bigint NOT NULL,
    requests integer NOT NULL,
    max_sent_bytes bigint NOT NULL,
    max_received_bytes bigint NOT NULL,
    max_requests integer NOT NULL,
    CONSTRAINT egress_counters CHECK (((sent_bytes >= 0) AND (received_bytes >= 0)))
);

ALTER TABLE ONLY public.egress_budgets FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.egress_requests (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    operation_id uuid,
    service text NOT NULL,
    method text NOT NULL,
    path text NOT NULL,
    query_digest text NOT NULL,
    status integer NOT NULL,
    sent_bytes bigint NOT NULL,
    received_bytes bigint NOT NULL,
    duration_ms bigint NOT NULL,
    error text,
    created_at timestamp with time zone NOT NULL,
    session_id uuid,
    via text NOT NULL,
    secret_id uuid,
    secret_version text,
    secret_host text,
    admission_ms bigint,
    settle_ms bigint,
    CONSTRAINT egress_counters CHECK (((sent_bytes >= 0) AND (received_bytes >= 0)))
);

ALTER TABLE ONLY public.egress_requests FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.egress_scope_connections (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    scope_id uuid NOT NULL,
    host text NOT NULL,
    port integer NOT NULL,
    sent_bytes bigint NOT NULL,
    received_bytes bigint NOT NULL,
    duration_ms bigint NOT NULL,
    outcome text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT egress_scope_connection_valid CHECK (((char_length(host) <= 255) AND (((port >= 1) AND (port <= 65535)) OR ((port = 0) AND (outcome = ANY (ARRAY['interrupted'::text, 'invalid_proxy_target'::text])))) AND (sent_bytes >= 0) AND (received_bytes >= 0) AND (duration_ms >= 0)))
);

ALTER TABLE ONLY public.egress_scope_connections FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.egress_scope_daily (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    scope_id uuid NOT NULL,
    day text NOT NULL,
    service text NOT NULL,
    sent_bytes bigint NOT NULL,
    received_bytes bigint NOT NULL,
    CONSTRAINT egress_scope_daily_nonnegative CHECK (((sent_bytes >= 0) AND (received_bytes >= 0)))
);

ALTER TABLE ONLY public.egress_scope_daily FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.egress_scope_events (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    scope_id uuid NOT NULL,
    kind text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    delivered_at timestamp with time zone
);

ALTER TABLE ONLY public.egress_scope_events FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.egress_scope_state (
    tenant text NOT NULL,
    node text NOT NULL,
    scope_id uuid NOT NULL,
    suspended boolean NOT NULL,
    window_started_at timestamp with time zone NOT NULL,
    connections_in_window integer NOT NULL,
    CONSTRAINT egress_scope_window_nonnegative CHECK ((connections_in_window >= 0))
);

ALTER TABLE ONLY public.egress_scope_state FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.egress_sessions (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    live_preview_id uuid NOT NULL,
    principal text NOT NULL,
    credential_id uuid NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    sent_bytes bigint DEFAULT 0 NOT NULL,
    received_bytes bigint DEFAULT 0 NOT NULL,
    requests integer DEFAULT 0 NOT NULL,
    max_sent_bytes bigint NOT NULL,
    max_received_bytes bigint NOT NULL,
    max_requests integer NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.egress_sessions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.estimate_calibration_samples (
    id uuid NOT NULL,
    upstream_model text NOT NULL,
    projection_version integer NOT NULL,
    byte_basis bigint NOT NULL,
    prompt_tokens bigint NOT NULL,
    completed_at timestamp with time zone NOT NULL,
    CONSTRAINT estimate_calibration_samples_counts CHECK (((projection_version > 0) AND (byte_basis > 0) AND (prompt_tokens > 0) AND ((length(upstream_model) >= 1) AND (length(upstream_model) <= 512))))
);

--

CREATE TABLE public.events (
    sequence bigint NOT NULL,
    tenant text NOT NULL,
    id uuid NOT NULL,
    node text,
    workspace_id uuid,
    kind text NOT NULL,
    data jsonb NOT NULL,
    published_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    publish_error text
);

ALTER TABLE ONLY public.events FORCE ROW LEVEL SECURITY;

--

ALTER TABLE public.events ALTER COLUMN sequence ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.events_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--

CREATE TABLE public.generation_budgets (
    request_id uuid NOT NULL,
    tenant text NOT NULL,
    token_limit bigint NOT NULL,
    used_tokens bigint DEFAULT 0 NOT NULL,
    compaction_call_limit bigint DEFAULT 0 NOT NULL,
    compaction_calls bigint DEFAULT 0 NOT NULL,
    embedding_call_limit bigint DEFAULT 0 NOT NULL,
    embedding_calls bigint DEFAULT 0 NOT NULL,
    summary_call_limit bigint DEFAULT 0 NOT NULL,
    summary_calls bigint DEFAULT 0 NOT NULL,
    CONSTRAINT embedding_calls_bounded CHECK ((embedding_calls <= embedding_call_limit)),
    CONSTRAINT generation_budgets_check CHECK (((used_tokens >= 0) AND (used_tokens <= token_limit))),
    CONSTRAINT generation_budgets_check1 CHECK (((compaction_calls >= 0) AND (compaction_calls <= compaction_call_limit))),
    CONSTRAINT generation_budgets_compaction_call_limit_check CHECK ((compaction_call_limit >= 0)),
    CONSTRAINT generation_budgets_embedding_call_limit_check CHECK ((embedding_call_limit >= 0)),
    CONSTRAINT generation_budgets_embedding_calls_check CHECK ((embedding_calls >= 0)),
    CONSTRAINT generation_budgets_summary_call_limit_check CHECK ((summary_call_limit >= 0)),
    CONSTRAINT generation_budgets_summary_calls_check CHECK (((summary_calls >= 0) AND (summary_calls <= summary_call_limit))),
    CONSTRAINT generation_budgets_token_limit_check CHECK ((token_limit > 0))
);

ALTER TABLE ONLY public.generation_budgets FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_compaction_usage (
    request_id uuid NOT NULL,
    tenant text NOT NULL,
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    request_bytes bigint NOT NULL,
    questions integer NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT generation_compaction_usage_questions_check CHECK ((questions > 0)),
    CONSTRAINT generation_compaction_usage_request_bytes_check CHECK ((request_bytes > 0))
);

ALTER TABLE ONLY public.generation_compaction_usage FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_embedding_usage (
    request_id uuid NOT NULL,
    tenant text NOT NULL,
    attempt_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    run_id uuid,
    entry_id uuid,
    purpose text NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    request_bytes bigint NOT NULL,
    reserved_tokens bigint NOT NULL,
    reported_tokens bigint,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT generation_embedding_usage_purpose_check CHECK ((purpose = ANY (ARRAY['query'::text, 'index'::text, 'reference'::text]))),
    CONSTRAINT generation_embedding_usage_reported_tokens_check CHECK ((reported_tokens >= 0)),
    CONSTRAINT generation_embedding_usage_request_bytes_check CHECK ((request_bytes > 0)),
    CONSTRAINT generation_embedding_usage_reserved_tokens_check CHECK ((reserved_tokens > 0))
);

ALTER TABLE ONLY public.generation_embedding_usage FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_history (
    sequence bigint NOT NULL,
    tenant text NOT NULL,
    request_id uuid NOT NULL,
    status text NOT NULL,
    actor text NOT NULL,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

ALTER TABLE ONLY public.generation_history FORCE ROW LEVEL SECURITY;

--

ALTER TABLE public.generation_history ALTER COLUMN sequence ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.generation_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--

CREATE TABLE public.generation_policies (
    tenant text NOT NULL,
    id text NOT NULL,
    revision bigint NOT NULL,
    spec jsonb NOT NULL,
    generated_count bigint DEFAULT 0 NOT NULL,
    allocated_tokens bigint DEFAULT 0 NOT NULL,
    allocated_compaction_calls bigint DEFAULT 0 NOT NULL,
    allocated_embedding_calls bigint DEFAULT 0 NOT NULL,
    allocated_summary_calls bigint DEFAULT 0 NOT NULL,
    CONSTRAINT generation_policies_allocated_compaction_calls_check CHECK ((allocated_compaction_calls >= 0)),
    CONSTRAINT generation_policies_allocated_embedding_calls_check CHECK ((allocated_embedding_calls >= 0)),
    CONSTRAINT generation_policies_allocated_summary_calls_check CHECK ((allocated_summary_calls >= 0)),
    CONSTRAINT generation_policies_allocated_tokens_check CHECK ((allocated_tokens >= 0)),
    CONSTRAINT generation_policies_generated_count_check CHECK ((generated_count >= 0)),
    CONSTRAINT generation_policies_revision_check CHECK ((revision > 0))
);

ALTER TABLE ONLY public.generation_policies FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_policy_history (
    tenant text NOT NULL,
    policy_id text NOT NULL,
    revision bigint NOT NULL,
    spec jsonb NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT generation_policy_history_revision CHECK (COALESCE((revision > 0), false))
);

ALTER TABLE ONLY public.generation_policy_history FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_remote_dispatches (
    attempt_id uuid NOT NULL,
    tenant text NOT NULL,
    usage jsonb NOT NULL,
    digest text NOT NULL,
    peer_node text NOT NULL,
    boundary jsonb NOT NULL,
    state text DEFAULT 'PREPARING'::text NOT NULL,
    reservations jsonb DEFAULT '[]'::jsonb NOT NULL,
    finalization jsonb,
    peer_finalized boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT generation_remote_dispatches_state_check CHECK ((state = ANY (ARRAY['PREPARING'::text, 'DISPATCHED'::text, 'SETTLED'::text, 'ABORTED'::text])))
);

ALTER TABLE ONLY public.generation_remote_dispatches FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_remote_finalizations (
    attempt_id uuid NOT NULL,
    tenant text NOT NULL,
    digest text NOT NULL,
    result jsonb
);

ALTER TABLE ONLY public.generation_remote_finalizations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_remote_intents (
    id uuid NOT NULL,
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    binding jsonb NOT NULL,
    cancelled boolean DEFAULT false NOT NULL,
    cancel_delivered boolean DEFAULT false NOT NULL,
    cancel_retry_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.generation_remote_intents FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_remote_usage (
    request_id uuid NOT NULL,
    tenant text NOT NULL,
    attempt_id uuid NOT NULL,
    operation_id uuid NOT NULL,
    dispatcher_node text NOT NULL,
    grant_id uuid NOT NULL,
    admission_id uuid NOT NULL,
    purpose text NOT NULL,
    digest text NOT NULL,
    reserved_tokens bigint NOT NULL,
    reported_tokens bigint,
    state text DEFAULT 'RESERVED'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT generation_remote_usage_purpose_check CHECK ((purpose = ANY (ARRAY['embedding'::text, 'inference'::text, 'compaction'::text, 'memory'::text, 'summary'::text]))),
    CONSTRAINT generation_remote_usage_reserved_tokens_check CHECK ((reserved_tokens >= 0)),
    CONSTRAINT generation_remote_usage_state_check CHECK ((state = ANY (ARRAY['RESERVED'::text, 'SETTLED'::text, 'RELEASED'::text])))
);

ALTER TABLE ONLY public.generation_remote_usage FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_requests (
    id uuid NOT NULL,
    tenant text NOT NULL,
    policy_id text NOT NULL,
    policy_revision bigint NOT NULL,
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    definition jsonb NOT NULL,
    status text NOT NULL,
    reason text NOT NULL,
    depth integer NOT NULL,
    token_limit bigint NOT NULL,
    quota_released boolean DEFAULT false NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    retired_catalog_revision bigint,
    home_node text NOT NULL,
    node text NOT NULL,
    foreign_intent jsonb,
    prepared boolean DEFAULT false NOT NULL,
    grant_id uuid,
    admission_id uuid,
    local_task_id uuid,
    local_workspace_id uuid,
    CONSTRAINT generation_foreign_binding CHECK ((((local_task_id IS NOT NULL) AND (foreign_intent IS NULL) AND (grant_id IS NULL) AND (admission_id IS NULL)) OR ((local_task_id IS NULL) AND (foreign_intent IS NOT NULL)))),
    CONSTRAINT generation_local_pair CHECK (((local_task_id IS NULL) = (local_workspace_id IS NULL))),
    CONSTRAINT generation_requests_depth_check CHECK (((depth >= 1) AND (depth <= 31))),
    CONSTRAINT generation_requests_status_check CHECK ((status = ANY (ARRAY['PENDING_APPROVAL'::text, 'QUEUED'::text, 'ACTIVE'::text, 'COMPLETED'::text, 'DENIED'::text, 'STOPPED'::text, 'EXPIRED'::text, 'FAILED'::text, 'DELETED'::text]))),
    CONSTRAINT generation_requests_subject_chain_check CHECK (((cardinality(subject_chain) >= 1) AND (cardinality(subject_chain) <= 31))),
    CONSTRAINT generation_requests_token_limit_check CHECK ((token_limit > 0))
);

ALTER TABLE ONLY public.generation_requests FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_summary_usage (
    request_id uuid NOT NULL,
    tenant text NOT NULL,
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    definition_digest text NOT NULL,
    request_bytes bigint NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT generation_summary_usage_request_bytes_check CHECK ((request_bytes > 0))
);

ALTER TABLE ONLY public.generation_summary_usage FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.generation_usage (
    request_id uuid NOT NULL,
    tenant text NOT NULL,
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL,
    reserved_tokens bigint NOT NULL,
    reported_tokens bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT generation_usage_reported_tokens_check CHECK ((reported_tokens >= 0)),
    CONSTRAINT generation_usage_reserved_tokens_check CHECK ((reserved_tokens > 0))
);

ALTER TABLE ONLY public.generation_usage FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.human_requests (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    run_id uuid NOT NULL,
    kind text NOT NULL,
    prompt text NOT NULL,
    response jsonb,
    request_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    answered_by text,
    CONSTRAINT human_requests_kind_check CHECK ((kind = ANY (ARRAY['QUESTION'::text, 'APPROVAL_REQUIRED'::text, 'CONFIRMATION'::text, 'INFORMATION_REQUEST'::text, 'MCP_ELICITATION'::text, 'MCP_CONNECTION'::text])))
);

ALTER TABLE ONLY public.human_requests FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.inbox (
    event_id uuid NOT NULL,
    received_at timestamp with time zone DEFAULT now() NOT NULL
);

--

CREATE TABLE public.inference_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    usage_attempt uuid NOT NULL,
    outcome text,
    reason text,
    first_seq bigint NOT NULL,
    last_seq bigint NOT NULL,
    started_at timestamp with time zone NOT NULL,
    finished_at timestamp with time zone,
    pruned_at timestamp with time zone,
    CONSTRAINT inference_attempt_outcome CHECK ((((outcome IS NULL) AND (reason IS NULL) AND (finished_at IS NULL) AND (pruned_at IS NULL)) OR ((outcome = ANY (ARRAY['accepted'::text, 'discarded'::text])) AND (reason IS NULL) AND (finished_at IS NOT NULL)) OR ((outcome = 'interrupted'::text) AND (reason = ANY (ARRAY['cancelled'::text, 'lease_lost'::text, 'stall'::text, 'stream_error'::text, 'revoked'::text])) AND (finished_at IS NOT NULL)))),
    CONSTRAINT inference_attempt_sequence CHECK (((first_seq >= 1) AND (last_seq >= first_seq)))
);

ALTER TABLE ONLY public.inference_attempts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.inference_progress (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    seq bigint NOT NULL,
    attempt_id uuid NOT NULL,
    kind text NOT NULL,
    item jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT inference_progress_bounds CHECK (((seq >= 1) AND (octet_length((item)::text) <= 16384))),
    CONSTRAINT inference_progress_kind CHECK ((kind = ANY (ARRAY['started'::text, 'text'::text, 'tool_call'::text, 'outcome'::text])))
);

ALTER TABLE ONLY public.inference_progress FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.inference_usage (
    attempt_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    lease_token uuid NOT NULL,
    response_epoch bigint NOT NULL,
    model_id text NOT NULL,
    model_version text NOT NULL,
    projection_version integer NOT NULL,
    input_tokens bigint,
    output_tokens bigint,
    cache_read_tokens bigint,
    cache_write_tokens bigint,
    reasoning_tokens bigint,
    cost_nanocredits bigint,
    upstream_cost_nanocredits bigint,
    estimated_tokens bigint NOT NULL,
    estimator text NOT NULL,
    estimator_version integer NOT NULL,
    estimate_confidence text NOT NULL,
    cache_identity text,
    outcome text NOT NULL,
    rejection_class text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    upstream_model text,
    byte_basis bigint,
    output_reservation bigint,
    estimate_ratio_ppm bigint,
    calibration_sample boolean DEFAULT false NOT NULL,
    purpose text NOT NULL,
    requested_route text,
    provider_identity jsonb,
    shadow_estimates jsonb,
    harness_contract integer NOT NULL,
    server_tool_use jsonb,
    CONSTRAINT inference_usage_counts CHECK (((projection_version > 0) AND (estimator_version > 0) AND (estimated_tokens >= 0) AND (input_tokens >= 0) AND (output_tokens >= 0) AND (cache_read_tokens >= 0) AND (cache_write_tokens >= 0) AND (reasoning_tokens >= 0) AND (cost_nanocredits >= 0) AND (upstream_cost_nanocredits >= 0))),
    CONSTRAINT inference_usage_estimate_confidence CHECK ((estimate_confidence = ANY (ARRAY['tokenized'::text, 'calibrated'::text, 'conservative'::text]))),
    CONSTRAINT inference_usage_estimate_parts CHECK (((byte_basis >= 0) AND (output_reservation >= 0) AND ((estimate_ratio_ppm >= 1) AND (estimate_ratio_ppm <= 1000000)) AND ((NOT calibration_sample) OR ((upstream_model IS NOT NULL) AND (byte_basis IS NOT NULL))))),
    CONSTRAINT inference_usage_harness_contract CHECK ((harness_contract > 0)),
    CONSTRAINT inference_usage_outcome CHECK (((outcome = ANY (ARRAY['dispatched'::text, 'completed'::text, 'rejected'::text, 'unknown'::text])) AND ((outcome = 'dispatched'::text) = (completed_at IS NULL)) AND ((outcome = 'rejected'::text) = (rejection_class IS NOT NULL)))),
    CONSTRAINT inference_usage_reported_only_when_completed CHECK (((outcome = 'completed'::text) OR (num_nonnulls(input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, cost_nanocredits, upstream_cost_nanocredits) = 0)))
);

ALTER TABLE ONLY public.inference_usage FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.invocations (
    idempotency_key text NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    tool text NOT NULL,
    input jsonb NOT NULL,
    status text NOT NULL,
    result jsonb,
    replay_safe boolean NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivery_failures integer DEFAULT 0 NOT NULL,
    delivery_pending boolean DEFAULT false NOT NULL,
    uncertain_reason text,
    tenant_secret_host text,
    tenant_secret_id uuid,
    tenant_secret_version text,
    connection_id uuid,
    CONSTRAINT invocations_status_check CHECK ((status = ANY (ARRAY['STARTED'::text, 'COMPLETED'::text, 'UNCERTAIN'::text, 'REFUSED'::text])))
);

ALTER TABLE ONLY public.invocations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.node_cancellation_jobs (
    id text NOT NULL,
    deletion_id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    kind text NOT NULL,
    work_id uuid NOT NULL,
    binding jsonb NOT NULL,
    retry_at timestamp with time zone NOT NULL,
    CONSTRAINT node_cancellation_jobs_kind CHECK ((kind = ANY (ARRAY['run'::text, 'admission'::text, 'generation'::text, 'grant'::text, 'intent'::text, 'task'::text, 'operation'::text, 'python'::text])))
);

ALTER TABLE ONLY public.node_cancellation_jobs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_account_terminations (
    account_id text NOT NULL,
    terminated_at timestamp with time zone NOT NULL,
    deletion_after_termination_days integer NOT NULL,
    recorded_at timestamp with time zone NOT NULL,
    CONSTRAINT research_account_terminations_period CHECK (((char_length(account_id) > 0) AND (deletion_after_termination_days > 0)))
);

--

CREATE TABLE public.research_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    invocation_key text NOT NULL,
    operation text NOT NULL,
    provider text NOT NULL,
    destination text NOT NULL,
    account_id text,
    price_micro_usd bigint,
    outcome text NOT NULL,
    result jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    retention_account_id text,
    retention_days integer,
    purged_at timestamp with time zone,
    purge_reason text,
    CONSTRAINT research_attempts_account_terms CHECK ((((retention_account_id IS NULL) = (retention_days IS NULL)) AND ((retention_days IS NULL) OR (retention_days > 0)) AND ((purged_at IS NULL) = (purge_reason IS NULL)) AND ((purge_reason IS NULL) OR (purge_reason = 'account_terms'::text)))),
    CONSTRAINT research_attempts_attribution CHECK ((((account_id IS NULL) = (price_micro_usd IS NULL)) AND ((price_micro_usd IS NULL) OR (price_micro_usd >= 0)))),
    CONSTRAINT research_attempts_operation CHECK ((operation = ANY (ARRAY['search'::text, 'fetch'::text]))),
    CONSTRAINT research_attempts_outcome CHECK (((outcome = ANY (ARRAY['dispatched'::text, 'completed'::text, 'failed'::text, 'unknown'::text])) AND ((outcome = 'dispatched'::text) = (completed_at IS NULL)) AND ((outcome = 'dispatched'::text) = (result IS NULL))))
);

ALTER TABLE ONLY public.research_attempts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_fragments (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    snapshot_id uuid NOT NULL,
    text text NOT NULL,
    line_start integer NOT NULL,
    line_end integer NOT NULL,
    pages jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    image_page integer,
    image_media_type text,
    image_width integer,
    image_height integer,
    image_digest text,
    passage_digest text,
    CONSTRAINT research_fragments_lines CHECK (((jsonb_typeof(pages) = 'array'::text) AND (((image_page IS NULL) AND (image_media_type IS NULL) AND (image_width IS NULL) AND (image_height IS NULL) AND (image_digest IS NULL) AND (line_start >= 1) AND (line_end >= line_start)) OR ((image_page >= 1) AND (line_start = 0) AND (line_end = 0) AND (image_media_type = 'image/png'::text) AND ((image_width >= 1) AND (image_width <= 1568)) AND ((image_height >= 1) AND (image_height <= 1568)) AND (image_digest ~~ 'sha256:%'::text)))))
);

ALTER TABLE ONLY public.research_fragments FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_journal_results (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    invocation_key text NOT NULL,
    digest text NOT NULL,
    reference jsonb NOT NULL,
    bytes bytea,
    purge_reason text,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT research_journal_results_payload CHECK ((((bytes IS NULL) OR (((octet_length(bytes) >= 1) AND (octet_length(bytes) <= 1048576)) AND (purge_reason IS NULL))) AND ((purge_reason IS NULL) OR (purge_reason = ANY (ARRAY['account_terms'::text, 'revocation'::text]))) AND (digest ~ '^sha256:[a-f0-9]{64}$'::text)))
);

ALTER TABLE ONLY public.research_journal_results FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_originals (
    snapshot_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    bytes bytea NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT research_originals_bytes CHECK (((octet_length(bytes) >= 1) AND (octet_length(bytes) <= 16777216)))
);

ALTER TABLE ONLY public.research_originals FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_page_images (
    fragment_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    bytes bytea NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT research_page_images_bytes CHECK (((octet_length(bytes) >= 1) AND (octet_length(bytes) <= 1048576)))
);

ALTER TABLE ONLY public.research_page_images FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_revocations (
    run_id uuid NOT NULL,
    tenant text,
    revoked_by jsonb NOT NULL,
    reason text NOT NULL,
    revoked_at timestamp with time zone NOT NULL,
    removed jsonb NOT NULL,
    CONSTRAINT research_revocations_record CHECK ((((char_length(reason) >= 1) AND (char_length(reason) <= 1000)) AND (jsonb_typeof(revoked_by) = 'object'::text) AND (jsonb_typeof(removed) = 'object'::text)))
);

ALTER TABLE ONLY public.research_revocations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_snapshots (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    source_id uuid NOT NULL,
    requested_url text NOT NULL,
    url text NOT NULL,
    title text,
    media_type text NOT NULL,
    fetched_at timestamp with time zone NOT NULL,
    raw_digest text NOT NULL,
    text_digest text NOT NULL,
    completeness text NOT NULL,
    lines jsonb NOT NULL,
    text_bytes bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    purged_at timestamp with time zone,
    purge_reason text,
    page_count integer,
    pages_without_text jsonb DEFAULT '[]'::jsonb NOT NULL,
    original_digest text,
    original_bytes bigint,
    original_retained boolean DEFAULT false NOT NULL,
    attempt_key text,
    CONSTRAINT research_snapshots_completeness CHECK ((completeness = ANY (ARRAY['complete'::text, 'text_limit'::text]))),
    CONSTRAINT research_snapshots_digests CHECK (((raw_digest ~~ 'sha256:%'::text) AND (text_digest ~~ 'sha256:%'::text) AND (text_bytes >= 0) AND (jsonb_typeof(lines) = 'array'::text))),
    CONSTRAINT research_snapshots_pages CHECK ((((page_count IS NULL) OR (page_count >= 1)) AND (jsonb_typeof(pages_without_text) = 'array'::text) AND ((original_digest IS NULL) = (original_bytes IS NULL)) AND ((original_digest IS NULL) OR (original_digest ~~ 'sha256:%'::text)) AND ((original_bytes IS NULL) OR ((original_bytes >= 1) AND (original_bytes <= 16777216))) AND ((NOT original_retained) OR ((original_digest IS NOT NULL) AND (purged_at IS NULL))))),
    CONSTRAINT research_snapshots_purge CHECK ((((purged_at IS NULL) = (purge_reason IS NULL)) AND ((purge_reason IS NULL) OR ((purge_reason = ANY (ARRAY['retention'::text, 'revocation'::text, 'account_terms'::text])) AND (text_bytes = 0) AND (lines = '[]'::jsonb)))))
);

ALTER TABLE ONLY public.research_snapshots FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.research_sources (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    url text NOT NULL,
    title text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    attempt_key text,
    CONSTRAINT research_sources_bounds CHECK ((((length(url) >= 1) AND (length(url) <= 4096)) AND (length(title) <= 512)))
);

ALTER TABLE ONLY public.research_sources FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.run_activations (
    generation bigint NOT NULL,
    tenant text NOT NULL,
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    run_id uuid NOT NULL,
    run_revision bigint NOT NULL,
    reason text NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    due_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    publication_epoch bigint DEFAULT 0 NOT NULL,
    publish_token uuid,
    publish_until timestamp with time zone,
    published_at timestamp with time zone,
    lease_token uuid,
    claimed_at timestamp with time zone,
    claim_source text,
    worker_pid bigint,
    disposition text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT run_activations_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'claimed'::text, 'deferred'::text, 'settled'::text])))
);

ALTER TABLE ONLY public.run_activations FORCE ROW LEVEL SECURITY;

--

CREATE SEQUENCE public.run_activations_generation_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--

ALTER SEQUENCE public.run_activations_generation_seq OWNED BY public.run_activations.generation;

--

CREATE TABLE public.run_context_events (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    seq bigint NOT NULL,
    origin text NOT NULL,
    event jsonb NOT NULL,
    digest text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT run_context_event_origin CHECK ((origin = ANY (ARRAY['appended'::text, 'imported'::text]))),
    CONSTRAINT run_context_event_seq CHECK ((seq > 0))
);

ALTER TABLE ONLY public.run_context_events FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.run_inputs (
    seq bigint NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    sender text NOT NULL,
    content text NOT NULL,
    message_id uuid,
    idempotency_key text NOT NULL,
    delivery_retry_at timestamp with time zone,
    reference_only boolean DEFAULT false NOT NULL
);

ALTER TABLE ONLY public.run_inputs FORCE ROW LEVEL SECURITY;

--

CREATE SEQUENCE public.run_inputs_seq_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--

ALTER SEQUENCE public.run_inputs_seq_seq OWNED BY public.run_inputs.seq;

--

CREATE TABLE public.run_safety_stops (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    action text NOT NULL,
    expected_control_revision bigint NOT NULL,
    actor_kind text NOT NULL,
    identity_id uuid,
    reason text NOT NULL,
    operation_id uuid NOT NULL,
    caller_key text NOT NULL,
    fingerprint text NOT NULL,
    outcome text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT run_safety_stops_record CHECK (((action = ANY (ARRAY['pause'::text, 'cancel'::text])) AND (outcome = ANY (ARRAY['accepted'::text, 'no_op'::text])) AND (actor_kind = ANY (ARRAY['shared_bearer'::text, 'browser'::text])) AND ((actor_kind = 'browser'::text) = (identity_id IS NOT NULL)) AND ((char_length(reason) >= 1) AND (char_length(reason) <= 1000)) AND (expected_control_revision >= 0)))
);

ALTER TABLE ONLY public.run_safety_stops FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.run_task_snapshots (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    task_revision bigint NOT NULL,
    step integer NOT NULL,
    input_seq bigint NOT NULL,
    body jsonb NOT NULL,
    captured_at timestamp with time zone NOT NULL,
    CONSTRAINT run_task_snapshot_bounds CHECK (((task_revision >= 0) AND (step >= 0) AND (input_seq >= 0)))
);

ALTER TABLE ONLY public.run_task_snapshots FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.runner_allocation_journals (
    deployment text NOT NULL,
    runner_instance uuid NOT NULL,
    pinned_at timestamp with time zone NOT NULL
);

--

CREATE TABLE public.runs (
    id uuid NOT NULL,
    tenant text NOT NULL,
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    home_node text NOT NULL,
    node text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    phase text DEFAULT 'READY'::text NOT NULL,
    control text DEFAULT 'ACTIVE'::text NOT NULL,
    context jsonb DEFAULT '{"usage": null, "history": [], "summary": "", "compactions": 0, "media_inferred_seq": 0, "run_message_summary": "", "message_read_coverage": {}, "run_message_summary_seq": 0, "message_inference_coverage": {}}'::jsonb NOT NULL,
    pending jsonb DEFAULT '{"data": {}, "recovery": {"retry": null}, "state_version": 1}'::jsonb NOT NULL,
    step integer DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    error text,
    lease_owner uuid,
    lease_until timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    observed_input_seq bigint DEFAULT 0 NOT NULL,
    ledger_worker_ready boolean DEFAULT false NOT NULL,
    pending_human_request_id uuid GENERATED ALWAYS AS (
CASE
    WHEN (((context -> 'binding_snapshot'::text) -> 'remote'::text) = 'true'::jsonb) THEN NULL::uuid
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
    WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = 'approval'::text) AND ((((pending -> 'data'::text) -> 'approval'::text) ->> 'source'::text) = 'home'::text) AND (jsonb_typeof((((pending -> 'data'::text) -> 'approval'::text) -> 'request_id'::text)) = 'string'::text) AND ((((pending -> 'data'::text) -> 'approval'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN ((((pending -> 'data'::text) -> 'approval'::text) ->> 'request_id'::text))::uuid
    ELSE NULL::uuid
END) STORED,
    semantic_retry_cycle integer DEFAULT 0 NOT NULL,
    control_revision bigint DEFAULT 0 NOT NULL,
    control_source text DEFAULT 'tenant'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    provider_container jsonb,
    CONSTRAINT runs_control_check CHECK ((control = ANY (ARRAY['ACTIVE'::text, 'PAUSED'::text, 'CANCELLED'::text]))),
    CONSTRAINT runs_control_provenance CHECK ((control_revision >= 0)),
    CONSTRAINT runs_control_source_model_enum_check CHECK ((control_source = ANY (ARRAY['deployment_administrator'::text, 'system'::text, 'tenant'::text]))),
    CONSTRAINT runs_counters CHECK (((step >= 0) AND (revision >= 0) AND (revision < '9223372036854775807'::bigint))),
    CONSTRAINT runs_lease CHECK (COALESCE((((lease_owner IS NULL) = (lease_until IS NULL)) AND ((lease_until IS NULL) OR isfinite(lease_until))), false)),
    CONSTRAINT runs_phase_check CHECK ((phase = ANY (ARRAY['READY'::text, 'THINKING'::text, 'TOOL_CALL'::text, 'WAITING'::text, 'COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text])))
);

ALTER TABLE ONLY public.runs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.subnode_limits (
    tenant text NOT NULL,
    members_per_subnode integer NOT NULL,
    subnodes_per_run integer NOT NULL,
    remote_runs_may_create boolean NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    updated_by text NOT NULL,
    CONSTRAINT subnode_limits_range CHECK ((((members_per_subnode >= 1) AND (members_per_subnode <= 20)) AND ((subnodes_per_run >= 1) AND (subnodes_per_run <= 10))))
);

ALTER TABLE ONLY public.subnode_limits FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.subnode_members (
    subnode_id uuid NOT NULL,
    agent_id text NOT NULL,
    tenant text NOT NULL,
    agent_version text NOT NULL,
    added_by_run_id uuid NOT NULL,
    added_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.subnode_members FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.subnode_operation_receipts (
    run_id uuid NOT NULL,
    call_key text NOT NULL,
    tenant text NOT NULL,
    fingerprint text NOT NULL,
    result jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.subnode_operation_receipts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.subnodes (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    creator_run_id uuid NOT NULL,
    creator_task_id uuid NOT NULL,
    creator_agent_id text NOT NULL,
    creator_agent_version text NOT NULL,
    leader_agent_id text NOT NULL,
    leader_agent_version text NOT NULL,
    allowance jsonb NOT NULL,
    workspace_id uuid NOT NULL,
    title text,
    state text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    ended_at timestamp with time zone,
    ended_reason text,
    creation_key text NOT NULL,
    creation_fingerprint text NOT NULL,
    CONSTRAINT subnodes_lifecycle CHECK ((((state = 'active'::text) AND (ended_at IS NULL) AND (ended_reason IS NULL)) OR ((state = 'dissolved'::text) AND (ended_at IS NOT NULL) AND (ended_reason IS NOT NULL))))
);

ALTER TABLE ONLY public.subnodes FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.url_intents (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    input_key text,
    pages jsonb NOT NULL,
    requested_by text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT url_intents_pages CHECK (((jsonb_typeof(pages) = 'array'::text) AND ((jsonb_array_length(pages) >= 1) AND (jsonb_array_length(pages) <= 5))))
);

ALTER TABLE ONLY public.url_intents FORCE ROW LEVEL SECURITY;

--

ALTER TABLE ONLY public.run_activations ALTER COLUMN generation SET DEFAULT nextval('public.run_activations_generation_seq'::regclass);

--

ALTER TABLE ONLY public.run_inputs ALTER COLUMN seq SET DEFAULT nextval('public.run_inputs_seq_seq'::regclass);

--

ALTER TABLE ONLY public.activation_quarantine
    ADD CONSTRAINT activation_quarantine_pkey PRIMARY KEY (digest);

--

ALTER TABLE ONLY public.approval_observation_attempts
    ADD CONSTRAINT approval_observation_attempts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.approval_observations
    ADD CONSTRAINT approval_observation_identity UNIQUE (tenant, approval_type, approval);

--

ALTER TABLE ONLY public.approval_observation_population
    ADD CONSTRAINT approval_observation_population_pkey PRIMARY KEY (approval);

--

ALTER TABLE ONLY public.approval_observations
    ADD CONSTRAINT approval_observation_tenant_id UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.approval_observations
    ADD CONSTRAINT approval_observations_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.approval_observer_settings
    ADD CONSTRAINT approval_observer_settings_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.context_compaction_attempts
    ADD CONSTRAINT context_compaction_attempts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_areas
    ADD CONSTRAINT core_areas_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_areas
    ADD CONSTRAINT core_areas_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.core_blob_references
    ADD CONSTRAINT core_blob_references_holder_key UNIQUE (tenant, digest, holder_kind, holder_id);

--

ALTER TABLE ONLY public.core_blob_references
    ADD CONSTRAINT core_blob_references_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_blob_uploads
    ADD CONSTRAINT core_blob_uploads_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_blobs
    ADD CONSTRAINT core_blobs_pkey PRIMARY KEY (tenant, digest);

--

ALTER TABLE ONLY public.core_blobs
    ADD CONSTRAINT core_blobs_tenant_digest_size_key UNIQUE (tenant, digest, size);

--

ALTER TABLE ONLY public.core_live_previews
    ADD CONSTRAINT core_live_previews_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_live_previews
    ADD CONSTRAINT core_live_previews_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.core_manifest_revisions
    ADD CONSTRAINT core_manifest_revisions_pkey PRIMARY KEY (tenant, area_id, revision);

--

ALTER TABLE ONLY public.core_objects
    ADD CONSTRAINT core_objects_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_objects
    ADD CONSTRAINT core_objects_tenant_id_digest_key UNIQUE (tenant, id, digest);

--

ALTER TABLE ONLY public.core_objects
    ADD CONSTRAINT core_objects_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.core_operations
    ADD CONSTRAINT core_operation_key UNIQUE (tenant, principal, request_key);

--

ALTER TABLE ONLY public.core_operations
    ADD CONSTRAINT core_operations_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_operations
    ADD CONSTRAINT core_operations_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.core_preview_limits
    ADD CONSTRAINT core_preview_limits_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.core_preview_sessions
    ADD CONSTRAINT core_preview_sessions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_preview_tickets
    ADD CONSTRAINT core_preview_tickets_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_previews
    ADD CONSTRAINT core_previews_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_previews
    ADD CONSTRAINT core_previews_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.core_quotas
    ADD CONSTRAINT core_quotas_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.core_records
    ADD CONSTRAINT core_records_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_records
    ADD CONSTRAINT core_records_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.core_requests
    ADD CONSTRAINT core_requests_pkey PRIMARY KEY (tenant, principal, key);

--

ALTER TABLE ONLY public.core_retention_files
    ADD CONSTRAINT core_retention_files_hold_file_key UNIQUE (hold_id, file_id);

--

ALTER TABLE ONLY public.core_retention_files
    ADD CONSTRAINT core_retention_files_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_retention_holds
    ADD CONSTRAINT core_retention_holds_holder_revision_key UNIQUE (tenant, area_id, holder_kind, holder_id, revision);

--

ALTER TABLE ONLY public.core_retention_holds
    ADD CONSTRAINT core_retention_holds_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.core_retention_holds
    ADD CONSTRAINT core_retention_holds_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_run_order UNIQUE (area_id, sequence);

--

ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_runs_pkey PRIMARY KEY (run_id);

--

ALTER TABLE ONLY public.core_areas
    ADD CONSTRAINT core_session_identity UNIQUE (tenant, home_node, workspace_id, thread_id, agent_id, owner);

--

ALTER TABLE ONLY public.core_task_sessions
    ADD CONSTRAINT core_task_sessions_pkey PRIMARY KEY (task_id);

--

ALTER TABLE ONLY public.decision_attempts
    ADD CONSTRAINT decision_attempts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.decision_evidence
    ADD CONSTRAINT decision_evidence_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.decision_evidence
    ADD CONSTRAINT decision_evidence_run_identity UNIQUE (run_id, id);

--

ALTER TABLE ONLY public.decision_evidence
    ADD CONSTRAINT decision_evidence_tenant_run_id_id_key UNIQUE (tenant, run_id, id);

--

ALTER TABLE ONLY public.decision_outcomes
    ADD CONSTRAINT decision_outcomes_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.decision_states
    ADD CONSTRAINT decision_state_evidence_once UNIQUE (evidence_id);

--

ALTER TABLE ONLY public.decision_states
    ADD CONSTRAINT decision_states_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.egress_budgets
    ADD CONSTRAINT egress_budgets_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.egress_requests
    ADD CONSTRAINT egress_requests_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.egress_scope_connections
    ADD CONSTRAINT egress_scope_connections_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.egress_scope_daily
    ADD CONSTRAINT egress_scope_daily_key UNIQUE (tenant, node, scope_id, day, service);

--

ALTER TABLE ONLY public.egress_scope_daily
    ADD CONSTRAINT egress_scope_daily_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.egress_scope_events
    ADD CONSTRAINT egress_scope_events_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.egress_scope_state
    ADD CONSTRAINT egress_scope_state_owner_key UNIQUE (tenant, node, scope_id);

--

ALTER TABLE ONLY public.egress_scope_state
    ADD CONSTRAINT egress_scope_state_pkey PRIMARY KEY (scope_id);

--

ALTER TABLE ONLY public.egress_sessions
    ADD CONSTRAINT egress_sessions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.egress_sessions
    ADD CONSTRAINT egress_sessions_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.estimate_calibration_samples
    ADD CONSTRAINT estimate_calibration_samples_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_id_key UNIQUE (id);

--

ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.generation_budgets
    ADD CONSTRAINT generation_budgets_pkey PRIMARY KEY (request_id);

--

ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_pkey PRIMARY KEY (request_id, attempt_id);

--

ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_pkey PRIMARY KEY (request_id, attempt_id);

--

ALTER TABLE ONLY public.generation_history
    ADD CONSTRAINT generation_history_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.generation_policies
    ADD CONSTRAINT generation_policies_pkey PRIMARY KEY (tenant, id);

--

ALTER TABLE ONLY public.generation_policy_history
    ADD CONSTRAINT generation_policy_history_pkey PRIMARY KEY (tenant, policy_id, revision);

--

ALTER TABLE ONLY public.generation_remote_dispatches
    ADD CONSTRAINT generation_remote_dispatches_pkey PRIMARY KEY (attempt_id);

--

ALTER TABLE ONLY public.generation_remote_finalizations
    ADD CONSTRAINT generation_remote_finalizations_pkey PRIMARY KEY (attempt_id);

--

ALTER TABLE ONLY public.generation_remote_intents
    ADD CONSTRAINT generation_remote_intents_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.generation_remote_usage
    ADD CONSTRAINT generation_remote_usage_pkey PRIMARY KEY (request_id, attempt_id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_agent_id_key UNIQUE (agent_id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.generation_summary_usage
    ADD CONSTRAINT generation_summary_usage_pkey PRIMARY KEY (request_id, attempt_id);

--

ALTER TABLE ONLY public.generation_usage
    ADD CONSTRAINT generation_usage_pkey PRIMARY KEY (request_id, attempt_id);

--

ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_id_run_id_key UNIQUE (id, run_id);

--

ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_request_key_key UNIQUE (request_key);

--

ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_tenant_id_run_id_key UNIQUE (tenant, id, run_id);

--

ALTER TABLE ONLY public.inbox
    ADD CONSTRAINT inbox_pkey PRIMARY KEY (event_id);

--

ALTER TABLE ONLY public.inference_attempts
    ADD CONSTRAINT inference_attempts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.inference_attempts
    ADD CONSTRAINT inference_attempts_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.inference_progress
    ADD CONSTRAINT inference_progress_pkey PRIMARY KEY (run_id, seq);

--

ALTER TABLE ONLY public.inference_usage
    ADD CONSTRAINT inference_usage_pkey PRIMARY KEY (attempt_id);

--

ALTER TABLE ONLY public.invocations
    ADD CONSTRAINT invocations_pkey PRIMARY KEY (idempotency_key);

--

ALTER TABLE ONLY public.node_cancellation_jobs
    ADD CONSTRAINT node_cancellation_jobs_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.research_account_terminations
    ADD CONSTRAINT research_account_terminations_pkey PRIMARY KEY (account_id);

--

ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_invocation UNIQUE (run_id, invocation_key);

--

ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_variant UNIQUE (run_id, invocation_key, digest);

--

ALTER TABLE ONLY public.research_originals
    ADD CONSTRAINT research_originals_pkey PRIMARY KEY (snapshot_id);

--

ALTER TABLE ONLY public.research_page_images
    ADD CONSTRAINT research_page_images_pkey PRIMARY KEY (fragment_id);

--

ALTER TABLE ONLY public.research_revocations
    ADD CONSTRAINT research_revocations_pkey PRIMARY KEY (run_id);

--

ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_run_identity UNIQUE (run_id, id);

--

ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_tenant_run_id_id_key UNIQUE (tenant, run_id, id);

--

ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_run_identity UNIQUE (run_id, id);

--

ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_tenant_run_id_id_key UNIQUE (tenant, run_id, id);

--

ALTER TABLE ONLY public.run_activations
    ADD CONSTRAINT run_activations_id_key UNIQUE (id);

--

ALTER TABLE ONLY public.run_activations
    ADD CONSTRAINT run_activations_pkey PRIMARY KEY (generation);

--

ALTER TABLE ONLY public.run_context_events
    ADD CONSTRAINT run_context_events_pkey PRIMARY KEY (run_id, seq);

--

ALTER TABLE ONLY public.run_inputs
    ADD CONSTRAINT run_inputs_pkey PRIMARY KEY (seq);

--

ALTER TABLE ONLY public.run_inputs
    ADD CONSTRAINT run_inputs_run_key UNIQUE (run_id, idempotency_key);

--

ALTER TABLE ONLY public.run_safety_stops
    ADD CONSTRAINT run_safety_stops_operation UNIQUE (tenant, caller_key, operation_id);

--

ALTER TABLE ONLY public.run_safety_stops
    ADD CONSTRAINT run_safety_stops_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.run_task_snapshots
    ADD CONSTRAINT run_task_snapshots_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.runner_allocation_journals
    ADD CONSTRAINT runner_allocation_journals_pkey PRIMARY KEY (deployment);

--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_home_node_task_id_key UNIQUE (home_node, task_id);

--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_tenant_id_workspace_id_key UNIQUE (tenant, id, workspace_id);

--

ALTER TABLE ONLY public.subnode_limits
    ADD CONSTRAINT subnode_limits_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.subnode_members
    ADD CONSTRAINT subnode_members_pkey PRIMARY KEY (subnode_id, agent_id);

--

ALTER TABLE ONLY public.subnode_operation_receipts
    ADD CONSTRAINT subnode_operation_receipts_pkey PRIMARY KEY (run_id, call_key);

--

ALTER TABLE ONLY public.subnodes
    ADD CONSTRAINT subnodes_creation_key UNIQUE (creator_run_id, creation_key);

--

ALTER TABLE ONLY public.subnodes
    ADD CONSTRAINT subnodes_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.subnodes
    ADD CONSTRAINT subnodes_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.subnodes
    ADD CONSTRAINT subnodes_workspace_key UNIQUE (workspace_id);

--

ALTER TABLE ONLY public.url_intents
    ADD CONSTRAINT url_intents_pkey PRIMARY KEY (id);

--

CREATE UNIQUE INDEX context_compaction_attempt_open ON public.context_compaction_attempts USING btree (run_id) WHERE (outcome IS NULL);

--

CREATE UNIQUE INDEX decision_evidence_final ON public.decision_evidence USING btree (decision_id) WHERE (outcome = ANY (ARRAY['applied'::text, 'staged'::text, 'shadow'::text, 'allowed'::text, 'blocked'::text]));

--

CREATE UNIQUE INDEX egress_budget_operation_service ON public.egress_budgets USING btree (tenant, operation_id, service);

--

CREATE UNIQUE INDEX generation_requests_home_task ON public.generation_requests USING btree (home_node, task_id) WHERE ((local_task_id IS NULL) AND (status = ANY (ARRAY['PENDING_APPROVAL'::text, 'QUEUED'::text, 'ACTIVE'::text])));

--

CREATE UNIQUE INDEX generation_requests_local_task ON public.generation_requests USING btree (local_task_id);

--

CREATE UNIQUE INDEX idx_core_preview_sessions_digest ON public.core_preview_sessions USING btree (digest);

--

CREATE UNIQUE INDEX idx_core_preview_tickets_digest ON public.core_preview_tickets USING btree (digest);

--

CREATE UNIQUE INDEX idx_core_previews_file_id ON public.core_previews USING btree (file_id);

--

CREATE UNIQUE INDEX idx_core_previews_label ON public.core_previews USING btree (label);

--

CREATE UNIQUE INDEX idx_core_previews_live_preview_id ON public.core_previews USING btree (live_preview_id);

--

CREATE UNIQUE INDEX inference_attempt_pending ON public.inference_attempts USING btree (run_id) WHERE (outcome IS NULL);

--

CREATE UNIQUE INDEX runs_id_workspace_unique ON public.runs USING btree (id, workspace_id);

--

ALTER TABLE ONLY public.approval_observation_attempts
    ADD CONSTRAINT approval_observation_attempt_owner FOREIGN KEY (tenant, observation) REFERENCES public.approval_observations(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.approval_observation_population
    ADD CONSTRAINT approval_observation_population_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.approval_observations
    ADD CONSTRAINT approval_observations_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.context_compaction_attempts
    ADD CONSTRAINT context_compaction_attempt_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_blob_references
    ADD CONSTRAINT core_blob_references_blob_fkey FOREIGN KEY (tenant, digest) REFERENCES public.core_blobs(tenant, digest) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.core_blob_references
    ADD CONSTRAINT core_blob_references_object_fkey FOREIGN KEY (tenant, object_id, digest) REFERENCES public.core_objects(tenant, id, digest) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.core_blob_references
    ADD CONSTRAINT core_blob_references_record_fkey FOREIGN KEY (tenant, record_id) REFERENCES public.core_records(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_blob_references
    ADD CONSTRAINT core_blob_references_revision_fkey FOREIGN KEY (tenant, area_id, revision) REFERENCES public.core_manifest_revisions(tenant, area_id, revision) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_live_previews
    ADD CONSTRAINT core_live_previews_area_fkey FOREIGN KEY (tenant, area_id) REFERENCES public.core_areas(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_live_previews
    ADD CONSTRAINT core_live_previews_run_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_manifest_revisions
    ADD CONSTRAINT core_manifest_revisions_area_fkey FOREIGN KEY (tenant, area_id) REFERENCES public.core_areas(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_objects
    ADD CONSTRAINT core_objects_blob_fkey FOREIGN KEY (tenant, digest, size) REFERENCES public.core_blobs(tenant, digest, size) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.core_preview_sessions
    ADD CONSTRAINT core_preview_sessions_preview_fkey FOREIGN KEY (tenant, preview_id) REFERENCES public.core_previews(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_preview_tickets
    ADD CONSTRAINT core_preview_tickets_preview_fkey FOREIGN KEY (tenant, preview_id) REFERENCES public.core_previews(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_retention_files
    ADD CONSTRAINT core_retention_files_hold_fkey FOREIGN KEY (tenant, hold_id) REFERENCES public.core_retention_holds(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_retention_files
    ADD CONSTRAINT core_retention_files_object_fkey FOREIGN KEY (tenant, file_id) REFERENCES public.core_objects(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_retention_holds
    ADD CONSTRAINT core_retention_holds_area_fkey FOREIGN KEY (tenant, area_id) REFERENCES public.core_areas(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.core_runs
    ADD CONSTRAINT core_runs_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.decision_attempts
    ADD CONSTRAINT decision_attempt_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.decision_evidence
    ADD CONSTRAINT decision_evidence_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.decision_outcomes
    ADD CONSTRAINT decision_outcome_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.decision_states
    ADD CONSTRAINT decision_state_evidence FOREIGN KEY (tenant, run_id, evidence_id) REFERENCES public.decision_evidence(tenant, run_id, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.egress_budgets
    ADD CONSTRAINT egress_budgets_operation_fkey FOREIGN KEY (tenant, operation_id) REFERENCES public.core_operations(tenant, id);

--

ALTER TABLE ONLY public.egress_requests
    ADD CONSTRAINT egress_requests_operation_fkey FOREIGN KEY (tenant, operation_id) REFERENCES public.core_operations(tenant, id);

--

ALTER TABLE ONLY public.egress_requests
    ADD CONSTRAINT egress_requests_session_fkey FOREIGN KEY (tenant, session_id) REFERENCES public.egress_sessions(tenant, id);

--

ALTER TABLE ONLY public.egress_scope_connections
    ADD CONSTRAINT egress_scope_connections_scope_fkey FOREIGN KEY (tenant, node, scope_id) REFERENCES public.egress_scope_state(tenant, node, scope_id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.egress_scope_daily
    ADD CONSTRAINT egress_scope_daily_scope_fkey FOREIGN KEY (tenant, node, scope_id) REFERENCES public.egress_scope_state(tenant, node, scope_id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.egress_scope_events
    ADD CONSTRAINT egress_scope_events_scope_fkey FOREIGN KEY (tenant, node, scope_id) REFERENCES public.egress_scope_state(tenant, node, scope_id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.egress_sessions
    ADD CONSTRAINT egress_sessions_preview_fkey FOREIGN KEY (tenant, live_preview_id) REFERENCES public.core_live_previews(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.generation_budgets
    ADD CONSTRAINT generation_budgets_request_id_fkey FOREIGN KEY (tenant, request_id) REFERENCES public.generation_requests(tenant, id);

--

ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_request_id_fkey FOREIGN KEY (tenant, request_id) REFERENCES public.generation_requests(tenant, id);

--

ALTER TABLE ONLY public.generation_compaction_usage
    ADD CONSTRAINT generation_compaction_usage_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_request_id_fkey FOREIGN KEY (tenant, request_id) REFERENCES public.generation_requests(tenant, id);

--

ALTER TABLE ONLY public.generation_embedding_usage
    ADD CONSTRAINT generation_embedding_usage_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.generation_history
    ADD CONSTRAINT generation_history_request_id_fkey FOREIGN KEY (tenant, request_id) REFERENCES public.generation_requests(tenant, id);

--

ALTER TABLE ONLY public.generation_policy_history
    ADD CONSTRAINT generation_policy_history_tenant_policy_id_fkey FOREIGN KEY (tenant, policy_id) REFERENCES public.generation_policies(tenant, id);

--

ALTER TABLE ONLY public.generation_requests
    ADD CONSTRAINT generation_requests_tenant_policy_id_policy_revision_fkey FOREIGN KEY (tenant, policy_id, policy_revision) REFERENCES public.generation_policy_history(tenant, policy_id, revision);

--

ALTER TABLE ONLY public.generation_summary_usage
    ADD CONSTRAINT generation_summary_usage_request_id_fkey FOREIGN KEY (tenant, request_id) REFERENCES public.generation_requests(tenant, id);

--

ALTER TABLE ONLY public.generation_summary_usage
    ADD CONSTRAINT generation_summary_usage_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.generation_usage
    ADD CONSTRAINT generation_usage_request_id_fkey FOREIGN KEY (tenant, request_id) REFERENCES public.generation_requests(tenant, id);

--

ALTER TABLE ONLY public.generation_usage
    ADD CONSTRAINT generation_usage_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.human_requests
    ADD CONSTRAINT human_requests_run_workspace FOREIGN KEY (tenant, run_id, workspace_id) REFERENCES public.runs(tenant, id, workspace_id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.inference_attempts
    ADD CONSTRAINT inference_attempt_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.inference_progress
    ADD CONSTRAINT inference_progress_attempt FOREIGN KEY (tenant, attempt_id) REFERENCES public.inference_attempts(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.inference_progress
    ADD CONSTRAINT inference_progress_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.inference_usage
    ADD CONSTRAINT inference_usage_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.invocations
    ADD CONSTRAINT invocations_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_snapshot FOREIGN KEY (tenant, run_id, snapshot_id) REFERENCES public.research_snapshots(tenant, run_id, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.research_originals
    ADD CONSTRAINT research_originals_snapshot FOREIGN KEY (tenant, run_id, snapshot_id) REFERENCES public.research_snapshots(tenant, run_id, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.research_page_images
    ADD CONSTRAINT research_page_images_fragment FOREIGN KEY (tenant, fragment_id) REFERENCES public.research_fragments(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_source FOREIGN KEY (tenant, run_id, source_id) REFERENCES public.research_sources(tenant, run_id, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.run_context_events
    ADD CONSTRAINT run_context_event_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.run_inputs
    ADD CONSTRAINT run_inputs_run_id_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.run_task_snapshots
    ADD CONSTRAINT run_task_snapshot_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_human_request_ref FOREIGN KEY (tenant, pending_human_request_id, id) REFERENCES public.human_requests(tenant, id, run_id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.subnode_members
    ADD CONSTRAINT subnode_members_added_by_fkey FOREIGN KEY (tenant, added_by_run_id) REFERENCES public.runs(tenant, id);

--

ALTER TABLE ONLY public.subnode_members
    ADD CONSTRAINT subnode_members_subnode_fkey FOREIGN KEY (tenant, subnode_id) REFERENCES public.subnodes(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.subnode_operation_receipts
    ADD CONSTRAINT subnode_operation_receipts_run_fkey FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.subnodes
    ADD CONSTRAINT subnodes_creator_run_fkey FOREIGN KEY (tenant, creator_run_id) REFERENCES public.runs(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.url_intents
    ADD CONSTRAINT url_intents_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

--

SELECT pg_catalog.setval('public.events_sequence_seq', 1, false);

--

SELECT pg_catalog.setval('public.generation_history_sequence_seq', 1, false);

--

SELECT pg_catalog.setval('public.run_activations_generation_seq', 1, false);

--

SELECT pg_catalog.setval('public.run_inputs_seq_seq', 1, false);
