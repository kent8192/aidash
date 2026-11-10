SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE FUNCTION public.atomic_immutable_decision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.id<>OLD.id OR NEW.digest<>OLD.digest OR NEW.manifest<>OLD.manifest
       OR (OLD.decision IS NOT NULL AND NEW.decision IS DISTINCT FROM OLD.decision)
       OR (OLD.visible AND NOT NEW.visible) OR (OLD.complete AND NOT NEW.complete) THEN
        RAISE EXCEPTION 'atomic transaction identity and decisions are immutable';
    END IF;
    RETURN NEW;
END;
$$;

--

CREATE FUNCTION public.atomic_write_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE pending uuid;
BEGIN
    IF current_setting('aidash.transaction_control',true) = 'authority'
       AND TG_TABLE_NAME IN ('authorization_bundles','authorization_revisions',
           'authorization_credentials','authorization_decisions',
           'authorization_peer_mappings','authorization_peer_mapping_history',
           'node_registrations','node_registration_history') THEN
        RETURN NULL;
    END IF;
    SELECT transaction_id INTO pending FROM atomic_gate WHERE singleton FOR SHARE NOWAIT;
    IF pending IS NOT NULL AND current_setting('aidash.atomic_transaction',true) IS DISTINCT FROM pending::text THEN
        RAISE EXCEPTION 'atomic transaction visibility pending' USING ERRCODE='55P03';
    END IF;
    RETURN NULL;
END;
$$;

--

CREATE FUNCTION public.gate_remote_task_terminal() RETURNS trigger
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

--

CREATE TABLE public.atomic_authority_attempts (
    transaction_id uuid NOT NULL,
    node_id text NOT NULL,
    outcome text
);

--

CREATE TABLE public.atomic_coordinators (
    id uuid NOT NULL,
    digest text NOT NULL,
    manifest jsonb NOT NULL,
    decision text,
    visible boolean DEFAULT false NOT NULL,
    complete boolean DEFAULT false NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT atomic_coordinators_check CHECK (((NOT visible) OR ((decision IS NOT NULL) AND (decision = 'COMMIT'::text)))),
    CONSTRAINT atomic_coordinators_check1 CHECK (((NOT complete) OR (decision IS NOT NULL))),
    CONSTRAINT atomic_coordinators_decision_check CHECK ((decision = ANY (ARRAY['COMMIT'::text, 'ABORT'::text])))
);

--

CREATE TABLE public.atomic_gate (
    singleton boolean DEFAULT true NOT NULL,
    transaction_id uuid,
    commit_epoch bigint DEFAULT 0 NOT NULL,
    CONSTRAINT atomic_gate_singleton_check CHECK ((singleton = true))
);

--

CREATE TABLE public.atomic_history (
    sequence bigint NOT NULL,
    transaction_id uuid NOT NULL,
    role text NOT NULL,
    phase text NOT NULL,
    detail text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT atomic_history_role_check CHECK ((role = ANY (ARRAY['coordinator'::text, 'participant'::text, 'trust'::text])))
);

--

CREATE SEQUENCE public.atomic_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--

ALTER SEQUENCE public.atomic_history_sequence_seq OWNED BY public.atomic_history.sequence;

--

CREATE TABLE public.atomic_participants (
    id uuid NOT NULL,
    coordinator text NOT NULL,
    digest text NOT NULL,
    manifest jsonb NOT NULL,
    phase text NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT atomic_participants_phase_check CHECK ((phase = ANY (ARRAY['RESERVED'::text, 'PREPARED'::text, 'APPLIED'::text, 'COMMITTED'::text, 'ABORTED'::text])))
);

--

CREATE TABLE public.atomic_preflights (
    id uuid NOT NULL,
    binding jsonb NOT NULL
);

--

CREATE TABLE public.atomic_subjects (
    id uuid NOT NULL,
    binding jsonb NOT NULL
);

--

CREATE TABLE public.atomic_votes (
    transaction_id uuid NOT NULL,
    node_id text NOT NULL,
    phase text DEFAULT 'PENDING'::text NOT NULL,
    CONSTRAINT atomic_votes_phase_check CHECK ((phase = ANY (ARRAY['PENDING'::text, 'RESERVED'::text, 'PREPARED'::text, 'APPLIED'::text, 'COMMITTED'::text, 'ABORTED'::text])))
);

--

CREATE TABLE public.authorization_peer_mapping_history (
    sequence bigint NOT NULL,
    node text NOT NULL,
    source_node text NOT NULL,
    source_subject text NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    actor text NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT authorization_peer_mapping_history_revision_check CHECK ((revision > 0))
);

ALTER TABLE ONLY public.authorization_peer_mapping_history FORCE ROW LEVEL SECURITY;

--

CREATE SEQUENCE public.authorization_peer_mapping_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--

ALTER SEQUENCE public.authorization_peer_mapping_history_sequence_seq OWNED BY public.authorization_peer_mapping_history.sequence;

--

CREATE TABLE public.authorization_peer_mappings (
    node text NOT NULL,
    source_node text NOT NULL,
    source_subject text NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    actor text NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT authorization_peer_mappings_revision_check CHECK ((revision > 0))
);

ALTER TABLE ONLY public.authorization_peer_mappings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_remote_admissions (
    id uuid NOT NULL,
    node text NOT NULL,
    source_node text NOT NULL,
    grant_id uuid NOT NULL,
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    subject_chain text[] NOT NULL,
    description jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    cancelled boolean DEFAULT false NOT NULL
);

ALTER TABLE ONLY public.authorization_remote_admissions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_remote_grant_reads (
    grant_id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL
);

ALTER TABLE ONLY public.authorization_remote_grant_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_remote_grants (
    id uuid NOT NULL,
    task_id uuid NOT NULL,
    task_revision bigint NOT NULL,
    workspace_id uuid NOT NULL,
    node text NOT NULL,
    node_id text NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    inspection jsonb NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked boolean DEFAULT false NOT NULL,
    semantic jsonb DEFAULT '{"mode": "disabled"}'::jsonb NOT NULL
);

ALTER TABLE ONLY public.authorization_remote_grants FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.delegations (
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    node_id text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    delivered boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    human_requests jsonb DEFAULT '[]'::jsonb NOT NULL,
    binding_snapshot jsonb
);

ALTER TABLE ONLY public.delegations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.deployment_key_rollovers (
    deployment text NOT NULL,
    state text NOT NULL
);

--

CREATE TABLE public.node_registration_history (
    sequence bigint NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    source_node text NOT NULL,
    source_endpoint text,
    source_public_key_pem text,
    source_key_id text,
    atomic_transactions boolean NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    source_dashboard_origin text,
    source_keys text,
    keys_revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT node_registration_history_counterpart_check CHECK ((((source_endpoint IS NULL) = (source_public_key_pem IS NULL)) AND ((source_public_key_pem IS NULL) = (source_key_id IS NULL)))),
    CONSTRAINT node_registration_history_receiving_keys CHECK (((keys_revision > 0) AND ((source_keys IS NULL) = (source_key_id IS NULL)))),
    CONSTRAINT node_registration_history_revision_check CHECK ((revision > 0))
);

ALTER TABLE ONLY public.node_registration_history FORCE ROW LEVEL SECURITY;

--

CREATE SEQUENCE public.node_registration_history_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--

ALTER SEQUENCE public.node_registration_history_sequence_seq OWNED BY public.node_registration_history.sequence;

--

CREATE TABLE public.node_registrations (
    tenant text NOT NULL,
    node text NOT NULL,
    source_node text NOT NULL,
    source_endpoint text,
    source_public_key_pem text,
    source_key_id text,
    atomic_transactions boolean NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    source_dashboard_origin text,
    source_keys text,
    keys_revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT node_registrations_counterpart_check CHECK ((((source_endpoint IS NULL) = (source_public_key_pem IS NULL)) AND ((source_public_key_pem IS NULL) = (source_key_id IS NULL)))),
    CONSTRAINT node_registrations_receiving_keys CHECK (((keys_revision > 0) AND ((source_keys IS NULL) = (source_key_id IS NULL)))),
    CONSTRAINT node_registrations_revision_check CHECK ((revision > 0))
);

ALTER TABLE ONLY public.node_registrations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.remote_run_message_fences (
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    idempotency_key text NOT NULL,
    content text NOT NULL,
    input_seq bigint,
    expires_at timestamp with time zone,
    consumed boolean DEFAULT false NOT NULL
);

ALTER TABLE ONLY public.remote_run_message_fences FORCE ROW LEVEL SECURITY;

--

ALTER TABLE ONLY public.atomic_history ALTER COLUMN sequence SET DEFAULT nextval('public.atomic_history_sequence_seq'::regclass);

--

ALTER TABLE ONLY public.authorization_peer_mapping_history ALTER COLUMN sequence SET DEFAULT nextval('public.authorization_peer_mapping_history_sequence_seq'::regclass);

--

ALTER TABLE ONLY public.node_registration_history ALTER COLUMN sequence SET DEFAULT nextval('public.node_registration_history_sequence_seq'::regclass);

--

ALTER TABLE ONLY public.atomic_authority_attempts
    ADD CONSTRAINT atomic_authority_attempts_pkey PRIMARY KEY (transaction_id, node_id);

--

ALTER TABLE ONLY public.atomic_coordinators
    ADD CONSTRAINT atomic_coordinators_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.atomic_gate
    ADD CONSTRAINT atomic_gate_pkey PRIMARY KEY (singleton);

--

ALTER TABLE ONLY public.atomic_history
    ADD CONSTRAINT atomic_history_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.atomic_participants
    ADD CONSTRAINT atomic_participants_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.atomic_preflights
    ADD CONSTRAINT atomic_preflights_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.atomic_subjects
    ADD CONSTRAINT atomic_subjects_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.atomic_votes
    ADD CONSTRAINT atomic_votes_pkey PRIMARY KEY (transaction_id, node_id);

--

ALTER TABLE ONLY public.authorization_peer_mapping_history
    ADD CONSTRAINT authorization_peer_mapping_history_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_pkey PRIMARY KEY (tenant, node, source_node, source_subject);

--

ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_node_source_node_grant_id_key UNIQUE (tenant, node, source_node, grant_id);

--

ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_node_source_node_task_id_key UNIQUE (tenant, node, source_node, task_id);

--

ALTER TABLE ONLY public.authorization_remote_admissions
    ADD CONSTRAINT authorization_remote_admissions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.authorization_remote_grant_reads
    ADD CONSTRAINT authorization_remote_grant_reads_pkey PRIMARY KEY (grant_id, workspace_id, resource_kind, resource_id);

--

ALTER TABLE ONLY public.authorization_remote_grants
    ADD CONSTRAINT authorization_remote_grants_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.authorization_remote_grants
    ADD CONSTRAINT authorization_remote_grants_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.delegations
    ADD CONSTRAINT delegations_pkey PRIMARY KEY (task_id);

--

ALTER TABLE ONLY public.deployment_key_rollovers
    ADD CONSTRAINT deployment_key_rollovers_pkey PRIMARY KEY (deployment);

--

ALTER TABLE ONLY public.node_registration_history
    ADD CONSTRAINT node_registration_history_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.node_registrations
    ADD CONSTRAINT node_registrations_pkey PRIMARY KEY (tenant, node, source_node);

--

ALTER TABLE ONLY public.remote_run_message_fences
    ADD CONSTRAINT remote_run_message_fences_pkey PRIMARY KEY (task_id, idempotency_key);

--

ALTER TABLE ONLY public.atomic_gate
    ADD CONSTRAINT atomic_gate_transaction_id_fkey FOREIGN KEY (transaction_id) REFERENCES public.atomic_participants(id);

--

ALTER TABLE ONLY public.atomic_votes
    ADD CONSTRAINT atomic_votes_transaction_id_fkey FOREIGN KEY (transaction_id) REFERENCES public.atomic_coordinators(id);

--

ALTER TABLE ONLY public.authorization_peer_mappings
    ADD CONSTRAINT authorization_peer_mappings_registration_fkey FOREIGN KEY (tenant, node, source_node) REFERENCES public.node_registrations(tenant, node, source_node);

--

ALTER TABLE ONLY public.authorization_remote_grant_reads
    ADD CONSTRAINT authorization_remote_grant_reads_grant_id_fkey FOREIGN KEY (tenant, grant_id) REFERENCES public.authorization_remote_grants(tenant, id) ON DELETE CASCADE;

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

SELECT pg_catalog.setval('public.atomic_history_sequence_seq', 1, false);

--

SELECT pg_catalog.setval('public.authorization_peer_mapping_history_sequence_seq', 1, false);

--

SELECT pg_catalog.setval('public.node_registration_history_sequence_seq', 1, false);
