SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

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

--

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

--

CREATE TABLE public.memory_bank_decay (
    bank_id uuid NOT NULL,
    tenant text NOT NULL,
    policy jsonb NOT NULL,
    activated_at timestamp with time zone,
    as_of timestamp with time zone,
    cursor uuid,
    next_job timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.memory_bank_decay FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_bank_policy_changes (
    operation_id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    digest text NOT NULL,
    applied_revision bigint NOT NULL,
    actor jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT memory_policy_change_revision CHECK (((applied_revision > 0) AND (applied_revision < '9223372036854775807'::bigint)))
);

ALTER TABLE ONLY public.memory_bank_policy_changes FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_bank_settings (
    bank_id uuid NOT NULL,
    tenant text NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    revision bigint NOT NULL,
    next_maintenance timestamp with time zone NOT NULL,
    recovery_eligible_at timestamp with time zone NOT NULL,
    CONSTRAINT memory_settings_revision CHECK (((revision > 0) AND (revision < '9223372036854775807'::bigint)))
);

ALTER TABLE ONLY public.memory_bank_settings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_banks (
    id uuid NOT NULL,
    home text NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    participant_id uuid,
    revision bigint NOT NULL,
    CONSTRAINT memory_bank_revision CHECK (((revision > 0) AND (revision < '9223372036854775807'::bigint)))
);

ALTER TABLE ONLY public.memory_banks FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_candidate_reviews (
    operation_id uuid NOT NULL,
    tenant text NOT NULL,
    candidate_id uuid NOT NULL,
    bank_id uuid NOT NULL,
    observed_revision bigint NOT NULL,
    digest text NOT NULL,
    actor text NOT NULL,
    outcome jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT memory_review_revision CHECK ((observed_revision > 0))
);

ALTER TABLE ONLY public.memory_candidate_reviews FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_candidates (
    id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    revision bigint NOT NULL,
    run jsonb NOT NULL,
    text text NOT NULL,
    kind text NOT NULL,
    learning text NOT NULL,
    verification text NOT NULL,
    occurred_start timestamp with time zone,
    occurred_end timestamp with time zone,
    entities jsonb NOT NULL,
    evidence jsonb NOT NULL,
    links jsonb NOT NULL,
    state text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    mental_model jsonb,
    CONSTRAINT memory_candidate_kind CHECK (((kind = ANY (ARRAY['world'::text, 'experience'::text])) AND (verification = 'unverified'::text) AND (revision > 0))),
    CONSTRAINT memory_candidate_state CHECK ((state = ANY (ARRAY['pending'::text, 'admitted'::text, 'rejected'::text, 'invalidated'::text])))
);

ALTER TABLE ONLY public.memory_candidates FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_dependencies (
    unit_id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    source_revision bigint NOT NULL,
    CONSTRAINT memory_dependency_kind CHECK (((source_kind = ANY (ARRAY['unit'::text, 'message'::text, 'artifact'::text, 'run'::text])) AND (source_revision > 0)))
);

ALTER TABLE ONLY public.memory_dependencies FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_engine_jobs (
    id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    kind text NOT NULL,
    input jsonb NOT NULL,
    authority jsonb NOT NULL,
    state text NOT NULL,
    attempts integer NOT NULL,
    claim uuid,
    next_attempt timestamp with time zone NOT NULL,
    last_error text,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT memory_job_state CHECK (((state = ANY (ARRAY['pending'::text, 'running'::text, 'complete'::text, 'blocked'::text, 'failed'::text])) AND (attempts >= 0) AND (kind = ANY (ARRAY['learn'::text, 'observation'::text, 'mental_model'::text]))))
);

ALTER TABLE ONLY public.memory_engine_jobs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_history (
    unit_id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    revision bigint NOT NULL,
    operation_id uuid NOT NULL,
    actor text NOT NULL,
    text text NOT NULL,
    kind text NOT NULL,
    learning text NOT NULL,
    verification text NOT NULL,
    occurred_start timestamp with time zone,
    occurred_end timestamp with time zone,
    entities jsonb NOT NULL,
    evidence jsonb NOT NULL,
    links jsonb NOT NULL,
    learned_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    deleted boolean NOT NULL,
    stale boolean NOT NULL,
    mental_model jsonb
);

ALTER TABLE ONLY public.memory_history FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_learning_discovery (
    deployment text NOT NULL,
    cursor_updated timestamp with time zone NOT NULL,
    cursor_id uuid NOT NULL,
    cycle_before timestamp with time zone NOT NULL
);

--

CREATE TABLE public.memory_model_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    operation_id uuid NOT NULL,
    ordinal integer NOT NULL,
    digest text NOT NULL,
    state text NOT NULL,
    tokens bigint NOT NULL,
    cost_micros bigint NOT NULL,
    output jsonb,
    created_at timestamp with time zone NOT NULL,
    cycle integer DEFAULT 0 NOT NULL,
    source_run_ids jsonb,
    reserved_tokens bigint,
    reserved_cost_micros bigint,
    provider_usage jsonb
);

ALTER TABLE ONLY public.memory_model_attempts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_model_operations (
    id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    digest text NOT NULL,
    calls bigint NOT NULL,
    tokens bigint NOT NULL,
    cost_micros bigint NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.memory_model_operations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_participants (
    id uuid NOT NULL,
    home text NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    principal text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    revision bigint NOT NULL,
    deleted boolean NOT NULL,
    CONSTRAINT memory_participant_revision CHECK ((revision > 0))
);

ALTER TABLE ONLY public.memory_participants FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_publications (
    id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    source_id uuid NOT NULL,
    source_revision bigint NOT NULL,
    revision bigint NOT NULL,
    authority jsonb NOT NULL,
    deleted boolean NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.memory_publications FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_purge_jobs (
    unit_id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    revision bigint NOT NULL,
    purge_after timestamp with time zone NOT NULL,
    backup_until timestamp with time zone NOT NULL,
    state text NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_error text,
    CONSTRAINT memory_purge_state CHECK (((state = ANY (ARRAY['pending'::text, 'purged'::text, 'failed'::text])) AND (attempts >= 0) AND (revision > 0) AND (purge_after <= backup_until)))
);

ALTER TABLE ONLY public.memory_purge_jobs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_receipts (
    operation_id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    digest text NOT NULL,
    outcome jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.memory_receipts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_receiver_caches (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    invalidated boolean NOT NULL,
    state text NOT NULL,
    attempts integer NOT NULL,
    max_attempts integer NOT NULL,
    next_attempt timestamp with time zone NOT NULL,
    last_error text,
    CONSTRAINT memory_receiver_cache_state CHECK (((state = ANY (ARRAY['pending'::text, 'purged'::text, 'failed'::text])) AND (attempts >= 0) AND (max_attempts > 0) AND (attempts <= max_attempts)))
);

ALTER TABLE ONLY public.memory_receiver_caches FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_remote_read_gates (
    grant_id uuid NOT NULL,
    tenant text NOT NULL
);

ALTER TABLE ONLY public.memory_remote_read_gates FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_remote_reads (
    grant_id uuid NOT NULL,
    tenant text NOT NULL,
    unit_id uuid NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT memory_remote_read_revision CHECK ((revision > 0))
);

ALTER TABLE ONLY public.memory_remote_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_run_bindings (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    participant_id uuid NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    participant_revision bigint NOT NULL
);

ALTER TABLE ONLY public.memory_run_bindings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_run_read_gates (
    run_id uuid NOT NULL,
    tenant text NOT NULL
);

ALTER TABLE ONLY public.memory_run_read_gates FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_run_reads (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    unit_id uuid NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT memory_read_revision CHECK ((revision > 0))
);

ALTER TABLE ONLY public.memory_run_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_task_participants (
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    participant_id uuid NOT NULL,
    participant_revision bigint NOT NULL
);

ALTER TABLE ONLY public.memory_task_participants FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_unit_origins (
    unit_id uuid NOT NULL,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    authority jsonb NOT NULL,
    origin_run uuid,
    CONSTRAINT memory_origin_revision CHECK ((revision > 0))
);

ALTER TABLE ONLY public.memory_unit_origins FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_unit_retention (
    unit_id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    deliveries bigint NOT NULL,
    last_delivered_at timestamp with time zone,
    reactivated_at timestamp with time zone,
    pinned boolean NOT NULL,
    dormant_policy jsonb,
    changed_at timestamp with time zone NOT NULL,
    CONSTRAINT retention_deliveries CHECK (((deliveries >= 0) AND (deliveries < '9223372036854775807'::bigint)))
);

ALTER TABLE ONLY public.memory_unit_retention FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_unit_run_origins (
    unit_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL
);

ALTER TABLE ONLY public.memory_unit_run_origins FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.memory_units (
    id uuid NOT NULL,
    tenant text NOT NULL,
    bank_id uuid NOT NULL,
    revision bigint NOT NULL,
    text text NOT NULL,
    kind text NOT NULL,
    learning text NOT NULL,
    verification text NOT NULL,
    occurred_start timestamp with time zone,
    occurred_end timestamp with time zone,
    entities jsonb NOT NULL,
    evidence jsonb NOT NULL,
    links jsonb NOT NULL,
    learned_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    deleted boolean NOT NULL,
    stale boolean NOT NULL,
    mental_model jsonb,
    CONSTRAINT memory_unit_kind CHECK ((kind = ANY (ARRAY['world'::text, 'experience'::text, 'observation'::text, 'mental_model'::text]))),
    CONSTRAINT memory_unit_learning CHECK ((learning = ANY (ARRAY['fact'::text, 'preference'::text, 'procedure'::text, 'failure'::text]))),
    CONSTRAINT memory_unit_occurrence CHECK ((((occurred_start IS NULL) AND (occurred_end IS NULL)) OR ((occurred_start IS NOT NULL) AND (occurred_end IS NOT NULL) AND (occurred_start <= occurred_end)))),
    CONSTRAINT memory_unit_provenance CHECK (((jsonb_typeof(entities) = 'array'::text) AND (jsonb_typeof(evidence) = 'array'::text) AND (jsonb_typeof(links) = 'array'::text) AND ((kind <> ALL (ARRAY['observation'::text, 'mental_model'::text])) OR ((verification = 'unverified'::text) AND (jsonb_array_length(evidence) > 0))))),
    CONSTRAINT memory_unit_revision CHECK (((revision > 0) AND (revision < '9223372036854775807'::bigint))),
    CONSTRAINT memory_unit_verification CHECK ((verification = ANY (ARRAY['unverified'::text, 'supported'::text, 'contradicted'::text])))
);

ALTER TABLE ONLY public.memory_units FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_collections (
    collection text NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    vector jsonb NOT NULL,
    retired boolean DEFAULT false NOT NULL,
    last_error text,
    cleaned_at timestamp with time zone,
    next_attempt timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.semantic_collections FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_entries (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    key text NOT NULL,
    source jsonb NOT NULL,
    agent text,
    metadata jsonb NOT NULL,
    revision bigint NOT NULL,
    point_id uuid NOT NULL,
    index_revision bigint NOT NULL,
    deleted boolean DEFAULT false NOT NULL,
    state text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    created_by text NOT NULL,
    authority jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    next_attempt timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT semantic_entries_authority CHECK (COALESCE(((jsonb_typeof(authority) = 'object'::text) AND (authority ? 'credential'::text) AND (jsonb_typeof((authority -> 'credential'::text)) = ANY (ARRAY['string'::text, 'null'::text])) AND (authority ? 'tenant'::text) AND (jsonb_typeof((authority -> 'tenant'::text)) = 'string'::text) AND (authority ? 'subject'::text) AND (jsonb_typeof((authority -> 'subject'::text)) = 'string'::text) AND (authority ? 'subjects'::text) AND (jsonb_typeof((authority -> 'subjects'::text)) = 'array'::text) AND (NOT jsonb_path_exists(authority, 'strict $."subjects"[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (((authority -> 'credential'::text) = 'null'::jsonb) OR ((authority ->> 'credential'::text) ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'::text))), false)),
    CONSTRAINT semantic_entries_counters CHECK ((COALESCE(((revision > 0) AND (index_revision > 0) AND (attempts >= 0) AND
CASE (source ->> 'kind'::text)
    WHEN 'memory'::text THEN (
    CASE
        WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'text'::text]) = '{}'::jsonb)
        ELSE false
    END AND (jsonb_typeof((source -> 'text'::text)) = 'string'::text) AND (deleted OR (length(btrim((source ->> 'text'::text), E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0)))
    WHEN 'artifact'::text THEN (
    CASE
        WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
        ELSE false
    END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
    WHEN 'message'::text THEN (
    CASE
        WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
        ELSE false
    END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
    WHEN 'unit'::text THEN (
    CASE
        WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
        ELSE false
    END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
    ELSE false
END), false) OR COALESCE(((revision > 0) AND (index_revision > 0) AND (attempts >= 0) AND (jsonb_typeof(source) = 'object'::text) AND ((source - ARRAY['kind'::text, 'area_id'::text, 'generation'::text, 'file_id'::text, 'path'::text]) = '{}'::jsonb) AND ((source ->> 'kind'::text) = 'working_image'::text) AND
CASE
    WHEN ((jsonb_typeof((source -> 'generation'::text)) = 'number'::text) AND ((source ->> 'generation'::text) ~ '^[1-9][0-9]{0,18}$'::text)) THEN (((source ->> 'generation'::text))::numeric <= ('9223372036854775807'::bigint)::numeric)
    ELSE false
END AND (jsonb_typeof((source -> 'area_id'::text)) = 'string'::text) AND ((source ->> 'area_id'::text) ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'::text) AND (jsonb_typeof((source -> 'file_id'::text)) = 'string'::text) AND ((source ->> 'file_id'::text) ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'::text) AND (jsonb_typeof((source -> 'path'::text)) = 'string'::text) AND ((octet_length((source ->> 'path'::text)) >= 1) AND (octet_length((source ->> 'path'::text)) <= 4096))), false))),
    CONSTRAINT semantic_entries_state_check CHECK ((state = ANY (ARRAY['PENDING'::text, 'READY'::text, 'ERROR'::text, 'REVOKED'::text, 'DELETED'::text, 'UNSUPPORTED'::text])))
);

ALTER TABLE ONLY public.semantic_entries FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_history (
    sequence bigint NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    entry_id uuid,
    revision bigint NOT NULL,
    state text NOT NULL,
    detail text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.semantic_history FORCE ROW LEVEL SECURITY;

--

CREATE SEQUENCE public.semantic_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--

ALTER SEQUENCE public.semantic_history_sequence_seq OWNED BY public.semantic_history.sequence;

--

CREATE TABLE public.semantic_image_ledgers (
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    slots jsonb NOT NULL,
    authorities jsonb NOT NULL,
    initialized boolean DEFAULT false NOT NULL,
    waiting_authority boolean DEFAULT false NOT NULL,
    next_authority_check timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT semantic_image_ledgers_shape CHECK (((jsonb_typeof(slots) = 'object'::text) AND (jsonb_typeof(authorities) = 'object'::text)))
);

ALTER TABLE ONLY public.semantic_image_ledgers FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_image_publications (
    sequence bigint NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    area_id uuid NOT NULL,
    generation bigint NOT NULL,
    changes jsonb NOT NULL,
    authority jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT semantic_image_publications_shape CHECK (((generation > 0) AND
CASE
    WHEN (jsonb_typeof(changes) = 'array'::text) THEN (jsonb_array_length(changes) > 0)
    ELSE false
END AND (jsonb_typeof(authority) = 'object'::text)))
);

ALTER TABLE ONLY public.semantic_image_publications FORCE ROW LEVEL SECURITY;

--

ALTER TABLE public.semantic_image_publications ALTER COLUMN sequence ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME public.semantic_image_publications_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--

CREATE TABLE public.semantic_indexes (
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    spec jsonb NOT NULL,
    collection text NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT knowledge_embedding_inputs CHECK ((((spec #> '{embedding,inputs}'::text[]) IS NULL) OR ((spec #> '{embedding,inputs}'::text[]) = ANY (ARRAY['["text"]'::jsonb, '["text", "image"]'::jsonb, '["image", "text"]'::jsonb])))),
    CONSTRAINT semantic_embedding_provider_keys CHECK ((NOT ((spec -> 'embedding'::text) ? 'credential_env'::text))),
    CONSTRAINT semantic_image_limits CHECK ((
CASE
    WHEN ((spec -> 'max_image_bytes'::text) IS NULL) THEN true
    WHEN ((jsonb_typeof((spec -> 'max_image_bytes'::text)) = 'number'::text) AND (((spec -> 'max_image_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((spec ->> 'max_image_bytes'::text))::numeric >= (1)::numeric) AND (((spec ->> 'max_image_bytes'::text))::numeric <= (67108864)::numeric))
    ELSE false
END AND
CASE
    WHEN ((spec -> 'max_images'::text) IS NULL) THEN true
    WHEN ((jsonb_typeof((spec -> 'max_images'::text)) = 'number'::text) AND (((spec -> 'max_images'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((spec ->> 'max_images'::text))::numeric >= (0)::numeric) AND (((spec ->> 'max_images'::text))::numeric <= (1024)::numeric))
    ELSE false
END)),
    CONSTRAINT semantic_indexes_revision CHECK (COALESCE(((revision > 0) AND (revision < '9223372036854775807'::bigint) AND (jsonb_typeof(spec) = 'object'::text) AND
CASE
    WHEN (jsonb_typeof(spec) = 'object'::text) THEN ((spec - ARRAY['embedding'::text, 'vector'::text, 'enabled'::text, 'auto_context'::text, 'max_sources'::text, 'max_results'::text, 'max_result_tokens'::text, 'max_input_bytes'::text, 'max_image_bytes'::text, 'max_images'::text]) = '{}'::jsonb)
    ELSE false
END AND (jsonb_typeof((spec -> 'enabled'::text)) = 'boolean'::text) AND (jsonb_typeof((spec -> 'auto_context'::text)) = 'boolean'::text) AND
CASE
    WHEN ((jsonb_typeof((spec -> 'max_sources'::text)) = 'number'::text) AND (((spec -> 'max_sources'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_sources'::text))::text)::numeric >= (1)::numeric) AND ((((spec -> 'max_sources'::text))::text)::numeric <= (1024)::numeric))
    ELSE false
END AND
CASE
    WHEN ((jsonb_typeof((spec -> 'max_results'::text)) = 'number'::text) AND (((spec -> 'max_results'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_results'::text))::text)::numeric >= (1)::numeric) AND ((((spec -> 'max_results'::text))::text)::numeric <= (20)::numeric))
    ELSE false
END AND
CASE
    WHEN ((jsonb_typeof((spec -> 'max_result_tokens'::text)) = 'number'::text) AND (((spec -> 'max_result_tokens'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_result_tokens'::text))::text)::numeric >= (128)::numeric) AND ((((spec -> 'max_result_tokens'::text))::text)::numeric <= (32768)::numeric))
    ELSE false
END AND
CASE
    WHEN ((jsonb_typeof((spec -> 'max_input_bytes'::text)) = 'number'::text) AND (((spec -> 'max_input_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_input_bytes'::text))::text)::numeric >= (128)::numeric) AND ((((spec -> 'max_input_bytes'::text))::text)::numeric <= (32768)::numeric))
    ELSE false
END AND
CASE
    WHEN (jsonb_typeof((spec -> 'embedding'::text)) = 'object'::text) THEN (((spec -> 'embedding'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text, 'provider_credential'::text, 'model'::text, 'model_version'::text, 'dimensions'::text, 'inputs'::text]) = '{}'::jsonb)
    ELSE false
END AND
CASE
    WHEN (jsonb_typeof((spec -> 'vector'::text)) = 'object'::text) THEN (((spec -> 'vector'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text]) = '{}'::jsonb)
    ELSE false
END AND ((spec #>> '{embedding,provider}'::text[]) = ANY (ARRAY['openai'::text, 'openrouter'::text])) AND public.aidash_valid_http_endpoint((spec #> '{embedding,endpoint}'::text[])) AND (((spec #> '{embedding,credential_env}'::text[]) IS NULL) OR ((spec #> '{embedding,credential_env}'::text[]) = 'null'::jsonb) OR ((jsonb_typeof((spec #> '{embedding,credential_env}'::text[])) = 'string'::text) AND ((spec #>> '{embedding,credential_env}'::text[]) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text))) AND ((spec #>> '{vector,provider}'::text[]) = 'postgres'::text) AND ((spec #>> '{vector,endpoint}'::text[]) = 'local'::text) AND (((spec #> '{vector,credential_env}'::text[]) IS NULL) OR ((spec #> '{vector,credential_env}'::text[]) = 'null'::jsonb)) AND ((jsonb_typeof((spec #> '{embedding,model}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model}'::text[]), E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length((spec #>> '{embedding,model}'::text[])) <= 256)) AND ((jsonb_typeof((spec #> '{embedding,model_version}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model_version}'::text[]), E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length((spec #>> '{embedding,model_version}'::text[])) <= 128)) AND
CASE
    WHEN ((jsonb_typeof((spec #> '{embedding,dimensions}'::text[])) = 'number'::text) AND (((spec #> '{embedding,dimensions}'::text[]))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric >= (1)::numeric) AND ((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric <= (8192)::numeric))
    ELSE false
END), false)),
    CONSTRAINT semantic_provider_credential_config CHECK (COALESCE((((spec #> '{embedding,provider_credential}'::text[]) IS NULL) OR ((spec #> '{embedding,provider_credential}'::text[]) = 'null'::jsonb) OR (((spec #> '{embedding,provider_credential}'::text[]) = '"openrouter"'::jsonb) AND ((spec #>> '{embedding,provider}'::text[]) = 'openrouter'::text) AND ((spec #>> '{embedding,endpoint}'::text[]) = 'https://openrouter.ai/api/v1'::text) AND (((spec #> '{embedding,credential_env}'::text[]) IS NULL) OR ((spec #> '{embedding,credential_env}'::text[]) = 'null'::jsonb)))), false))
);

ALTER TABLE ONLY public.semantic_indexes FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_points (
    id uuid NOT NULL,
    tenant text NOT NULL,
    entry_id uuid NOT NULL,
    content_digest text,
    collection text NOT NULL,
    retired boolean DEFAULT false NOT NULL,
    last_error text,
    cleaned_at timestamp with time zone,
    next_attempt timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.semantic_points FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_remote_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    operation_id uuid NOT NULL,
    fence bigint NOT NULL,
    cycle integer NOT NULL,
    state text NOT NULL,
    reservations jsonb DEFAULT '[]'::jsonb NOT NULL,
    error text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    dispatched_at timestamp with time zone,
    completed_at timestamp with time zone,
    CONSTRAINT semantic_remote_attempts_state_check CHECK ((state = ANY (ARRAY['RESERVING'::text, 'DISPATCHED'::text, 'COMPLETED'::text, 'ABORTED'::text, 'UNCERTAIN'::text])))
);

ALTER TABLE ONLY public.semantic_remote_attempts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_remote_operations (
    id uuid NOT NULL,
    tenant text NOT NULL,
    home_node text NOT NULL,
    grant_id uuid NOT NULL,
    admission_id uuid NOT NULL,
    digest text NOT NULL,
    binding jsonb NOT NULL,
    state text DEFAULT 'PENDING'::text NOT NULL,
    cycle integer DEFAULT 0 NOT NULL,
    failures integer DEFAULT 0 NOT NULL,
    attempt_id uuid,
    fence bigint DEFAULT 0 NOT NULL,
    lease_until timestamp with time zone,
    next_attempt timestamp with time zone,
    error text,
    receipt jsonb,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    corpus_digest text,
    CONSTRAINT semantic_remote_operations_state_check CHECK ((state = ANY (ARRAY['PENDING'::text, 'ACTIVE'::text, 'WAITING'::text, 'READY'::text, 'PAUSED'::text, 'INVALIDATED'::text, 'CANCELLED'::text])))
);

ALTER TABLE ONLY public.semantic_remote_operations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_remote_reads (
    grant_id uuid NOT NULL,
    tenant text NOT NULL,
    admission_id uuid NOT NULL,
    entry_id uuid NOT NULL,
    revision bigint NOT NULL,
    content_digest text NOT NULL
);

ALTER TABLE ONLY public.semantic_remote_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_remote_receipts (
    operation_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    digest text NOT NULL,
    receipt jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.semantic_remote_receipts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_run_reads (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    entry_id uuid NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT semantic_run_reads_revision CHECK (COALESCE((revision > 0), false))
);

ALTER TABLE ONLY public.semantic_run_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.semantic_vector_collections (
    collection text NOT NULL,
    dimensions integer NOT NULL,
    CONSTRAINT semantic_vector_dimensions CHECK (((dimensions >= 1) AND (dimensions <= 8192)))
);

--

CREATE TABLE public.semantic_vectors (
    id uuid NOT NULL,
    collection text NOT NULL,
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    embedding public.vector NOT NULL,
    payload jsonb NOT NULL
);

ALTER TABLE ONLY public.semantic_vectors FORCE ROW LEVEL SECURITY;

--

ALTER TABLE ONLY public.semantic_history ALTER COLUMN sequence SET DEFAULT nextval('public.semantic_history_sequence_seq'::regclass);

--

ALTER TABLE ONLY public.memory_bank_decay
    ADD CONSTRAINT memory_bank_decay_pkey PRIMARY KEY (bank_id);

--

ALTER TABLE ONLY public.memory_bank_policy_changes
    ADD CONSTRAINT memory_bank_policy_changes_pkey PRIMARY KEY (operation_id);

--

ALTER TABLE ONLY public.memory_bank_settings
    ADD CONSTRAINT memory_bank_settings_pkey PRIMARY KEY (bank_id);

--

ALTER TABLE ONLY public.memory_banks
    ADD CONSTRAINT memory_banks_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_banks
    ADD CONSTRAINT memory_banks_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.memory_candidate_reviews
    ADD CONSTRAINT memory_candidate_reviews_pkey PRIMARY KEY (operation_id);

--

ALTER TABLE ONLY public.memory_candidates
    ADD CONSTRAINT memory_candidates_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_dependencies
    ADD CONSTRAINT memory_dependencies_pk PRIMARY KEY (unit_id, source_kind, source_id, source_revision);

--

ALTER TABLE ONLY public.memory_engine_jobs
    ADD CONSTRAINT memory_engine_jobs_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_history
    ADD CONSTRAINT memory_history_pk PRIMARY KEY (unit_id, revision);

--

ALTER TABLE ONLY public.memory_learning_discovery
    ADD CONSTRAINT memory_learning_discovery_pkey PRIMARY KEY (deployment);

--

ALTER TABLE ONLY public.memory_model_attempts
    ADD CONSTRAINT memory_model_attempts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_model_operations
    ADD CONSTRAINT memory_model_operations_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_model_operations
    ADD CONSTRAINT memory_model_operations_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.memory_participants
    ADD CONSTRAINT memory_participants_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_participants
    ADD CONSTRAINT memory_participants_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.memory_publications
    ADD CONSTRAINT memory_publications_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_purge_jobs
    ADD CONSTRAINT memory_purge_jobs_pkey PRIMARY KEY (unit_id);

--

ALTER TABLE ONLY public.memory_receipts
    ADD CONSTRAINT memory_receipts_pkey PRIMARY KEY (operation_id);

--

ALTER TABLE ONLY public.memory_receiver_caches
    ADD CONSTRAINT memory_receiver_caches_pkey PRIMARY KEY (run_id);

--

ALTER TABLE ONLY public.memory_remote_read_gates
    ADD CONSTRAINT memory_remote_read_gates_pkey PRIMARY KEY (grant_id);

--

ALTER TABLE ONLY public.memory_remote_reads
    ADD CONSTRAINT memory_remote_reads_pkey PRIMARY KEY (grant_id, unit_id, revision);

--

ALTER TABLE ONLY public.memory_run_bindings
    ADD CONSTRAINT memory_run_bindings_pkey PRIMARY KEY (run_id);

--

ALTER TABLE ONLY public.memory_run_read_gates
    ADD CONSTRAINT memory_run_read_gates_pkey PRIMARY KEY (run_id);

--

ALTER TABLE ONLY public.memory_run_reads
    ADD CONSTRAINT memory_run_reads_pkey PRIMARY KEY (run_id, unit_id, revision);

--

ALTER TABLE ONLY public.memory_task_participants
    ADD CONSTRAINT memory_task_participants_pkey PRIMARY KEY (task_id);

--

ALTER TABLE ONLY public.memory_unit_origins
    ADD CONSTRAINT memory_unit_origins_pkey PRIMARY KEY (unit_id);

--

ALTER TABLE ONLY public.memory_unit_retention
    ADD CONSTRAINT memory_unit_retention_pkey PRIMARY KEY (unit_id);

--

ALTER TABLE ONLY public.memory_unit_run_origins
    ADD CONSTRAINT memory_unit_run_origins_pkey PRIMARY KEY (unit_id, run_id);

--

ALTER TABLE ONLY public.memory_units
    ADD CONSTRAINT memory_units_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.memory_units
    ADD CONSTRAINT memory_units_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.semantic_collections
    ADD CONSTRAINT semantic_collections_pkey PRIMARY KEY (collection);

--

ALTER TABLE ONLY public.semantic_collections
    ADD CONSTRAINT semantic_collections_tenant_collection_key UNIQUE (tenant, collection);

--

ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_workspace_id_key_key UNIQUE (workspace_id, key);

--

ALTER TABLE ONLY public.semantic_history
    ADD CONSTRAINT semantic_history_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.semantic_image_ledgers
    ADD CONSTRAINT semantic_image_ledgers_pkey PRIMARY KEY (workspace_id);

--

ALTER TABLE ONLY public.semantic_image_publications
    ADD CONSTRAINT semantic_image_publications_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_collection_key UNIQUE (collection);

--

ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_pkey PRIMARY KEY (workspace_id);

--

ALTER TABLE ONLY public.semantic_indexes
    ADD CONSTRAINT semantic_indexes_tenant_workspace_id_key UNIQUE (tenant, workspace_id);

--

ALTER TABLE ONLY public.semantic_points
    ADD CONSTRAINT semantic_points_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.semantic_remote_attempts
    ADD CONSTRAINT semantic_remote_attempts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.semantic_remote_operations
    ADD CONSTRAINT semantic_remote_operations_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.semantic_remote_reads
    ADD CONSTRAINT semantic_remote_reads_pkey PRIMARY KEY (grant_id, admission_id, entry_id, revision);

--

ALTER TABLE ONLY public.semantic_remote_receipts
    ADD CONSTRAINT semantic_remote_receipts_pkey PRIMARY KEY (operation_id);

--

ALTER TABLE ONLY public.semantic_run_reads
    ADD CONSTRAINT semantic_run_reads_pkey PRIMARY KEY (run_id, entry_id, revision);

--

ALTER TABLE ONLY public.semantic_vector_collections
    ADD CONSTRAINT semantic_vector_collections_pkey PRIMARY KEY (collection);

--

ALTER TABLE ONLY public.semantic_vectors
    ADD CONSTRAINT semantic_vectors_pkey PRIMARY KEY (id);

--

CREATE UNIQUE INDEX memory_bank_scope ON public.memory_banks USING btree (home, tenant, workspace_id, participant_id) NULLS NOT DISTINCT;

--

ALTER TABLE ONLY public.memory_bank_decay
    ADD CONSTRAINT decay_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_task_participants
    ADD CONSTRAINT memory_assignment_participant FOREIGN KEY (tenant, participant_id) REFERENCES public.memory_participants(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_model_attempts
    ADD CONSTRAINT memory_attempt_operation FOREIGN KEY (tenant, operation_id) REFERENCES public.memory_model_operations(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_banks
    ADD CONSTRAINT memory_bank_participant FOREIGN KEY (tenant, participant_id) REFERENCES public.memory_participants(tenant, id);

--

ALTER TABLE ONLY public.memory_run_bindings
    ADD CONSTRAINT memory_binding_participant FOREIGN KEY (tenant, participant_id) REFERENCES public.memory_participants(tenant, id);

--

ALTER TABLE ONLY public.memory_candidates
    ADD CONSTRAINT memory_candidate_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id);

--

ALTER TABLE ONLY public.memory_dependencies
    ADD CONSTRAINT memory_dependency_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id);

--

ALTER TABLE ONLY public.memory_dependencies
    ADD CONSTRAINT memory_dependency_unit FOREIGN KEY (tenant, unit_id) REFERENCES public.memory_units(tenant, id);

--

ALTER TABLE ONLY public.memory_history
    ADD CONSTRAINT memory_history_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id);

--

ALTER TABLE ONLY public.memory_engine_jobs
    ADD CONSTRAINT memory_job_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_model_operations
    ADD CONSTRAINT memory_operation_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_unit_origins
    ADD CONSTRAINT memory_origin_unit FOREIGN KEY (tenant, unit_id) REFERENCES public.memory_units(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_bank_policy_changes
    ADD CONSTRAINT memory_policy_change_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_publications
    ADD CONSTRAINT memory_publication_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.memory_publications
    ADD CONSTRAINT memory_publication_source FOREIGN KEY (tenant, source_id) REFERENCES public.memory_units(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_purge_jobs
    ADD CONSTRAINT memory_purge_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_run_reads
    ADD CONSTRAINT memory_read_unit FOREIGN KEY (tenant, unit_id) REFERENCES public.memory_units(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_receipts
    ADD CONSTRAINT memory_receipt_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id);

--

ALTER TABLE ONLY public.memory_candidate_reviews
    ADD CONSTRAINT memory_review_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_bank_settings
    ADD CONSTRAINT memory_settings_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_units
    ADD CONSTRAINT memory_unit_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id);

--

ALTER TABLE ONLY public.memory_unit_run_origins
    ADD CONSTRAINT memory_unit_run_origin_unit FOREIGN KEY (tenant, unit_id) REFERENCES public.memory_units(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_unit_retention
    ADD CONSTRAINT retention_bank FOREIGN KEY (tenant, bank_id) REFERENCES public.memory_banks(tenant, id) ON UPDATE RESTRICT ON DELETE RESTRICT;

--

ALTER TABLE ONLY public.memory_unit_retention
    ADD CONSTRAINT retention_unit FOREIGN KEY (tenant, unit_id) REFERENCES public.memory_units(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.semantic_collections
    ADD CONSTRAINT semantic_collections_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.semantic_indexes(tenant, workspace_id);

--

ALTER TABLE ONLY public.semantic_entries
    ADD CONSTRAINT semantic_entries_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.semantic_indexes(tenant, workspace_id);

--

ALTER TABLE ONLY public.semantic_points
    ADD CONSTRAINT semantic_points_collection_fkey FOREIGN KEY (tenant, collection) REFERENCES public.semantic_collections(tenant, collection);

--

ALTER TABLE ONLY public.semantic_points
    ADD CONSTRAINT semantic_points_entry_id_fkey FOREIGN KEY (tenant, entry_id) REFERENCES public.semantic_entries(tenant, id);

--

ALTER TABLE ONLY public.semantic_run_reads
    ADD CONSTRAINT semantic_run_reads_entry_id_fkey FOREIGN KEY (tenant, entry_id) REFERENCES public.semantic_entries(tenant, id);

--

ALTER TABLE ONLY public.semantic_vectors
    ADD CONSTRAINT semantic_vectors_collection_fk FOREIGN KEY (collection) REFERENCES public.semantic_vector_collections(collection) ON DELETE CASCADE;

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

SELECT pg_catalog.setval('public.semantic_history_sequence_seq', 1, false);

--

SELECT pg_catalog.setval('public.semantic_image_publications_sequence_seq', 1, false);


--
-- PostgreSQL database dump complete
--
