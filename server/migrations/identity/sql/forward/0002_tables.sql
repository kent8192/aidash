SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE FUNCTION public.aidash_deployment_scope() RETURNS boolean
    LANGUAGE plpgsql STABLE PARALLEL SAFE
    SET search_path TO 'pg_catalog'
    AS $$
DECLARE
    scope text := coalesce(current_setting('aidash.scope', true), '');
BEGIN
    IF scope = 'deployment' THEN
        RETURN true;
    ELSIF scope = 'tenant' THEN
        RETURN false;
    ELSIF scope = 'none' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is none';
    ELSIF scope = '' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unset';
    ELSE
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unknown';
    END IF;
END
$$;

--

CREATE FUNCTION public.aidash_node_deletion_immutable() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog'
    AS $$
BEGIN
    IF (NEW.id, NEW.tenant, NEW.node, NEW.deleted_at, NEW.deleted_by_kind,
        NEW.deleted_by_identity, NEW.purge_after) IS DISTINCT FROM
       (OLD.id, OLD.tenant, OLD.node, OLD.deleted_at, OLD.deleted_by_kind,
        OLD.deleted_by_identity, OLD.purge_after) THEN
        RAISE EXCEPTION 'Node deletion identity and deadline are immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$$;

--

CREATE FUNCTION public.aidash_scope_tenant() RETURNS text
    LANGUAGE plpgsql STABLE PARALLEL SAFE
    SET search_path TO 'pg_catalog'
    AS $$
DECLARE
    scope text := coalesce(current_setting('aidash.scope', true), '');
    tenant text := coalesce(current_setting('aidash.tenant', true), '');
BEGIN
    IF scope = 'tenant' THEN
        IF tenant = '' THEN
            RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
                USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is tenant without a Tenant name';
        END IF;
        RETURN tenant;
    ELSIF scope = 'deployment' THEN
        RETURN NULL;
    ELSIF scope = 'none' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is none';
    ELSIF scope = '' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unset';
    ELSE
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unknown';
    END IF;
END
$$;

--

CREATE FUNCTION public.aidash_tenant_purge() RETURNS boolean
    LANGUAGE sql STABLE PARALLEL SAFE
    SET search_path TO 'pg_catalog'
    AS $$
    SELECT coalesce(current_setting('aidash.scope', true), '') = 'deployment'
       AND coalesce(current_setting('application_name', true), '') = 'deployment:tenant-purge'
$$;

--

CREATE TABLE public.authorization_bundles (
    tenant text NOT NULL,
    revision bigint NOT NULL,
    document jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT authorization_bundles_revision_check CHECK ((revision > 0))
);

ALTER TABLE ONLY public.authorization_bundles FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_catalog (
    tenant text NOT NULL,
    node text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL,
    restriction jsonb,
    CONSTRAINT authorization_catalog_revision_check CHECK ((revision > 0))
);

ALTER TABLE ONLY public.authorization_catalog FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_catalog_history (
    tenant text NOT NULL,
    node text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    revision bigint NOT NULL,
    enabled boolean NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    restriction jsonb,
    provider_warning_acknowledgment text
);

ALTER TABLE ONLY public.authorization_catalog_history FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_credentials (
    id uuid NOT NULL,
    tenant text NOT NULL,
    subject text NOT NULL,
    token_hash bytea NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    issued_by text NOT NULL,
    mapping_id uuid,
    CONSTRAINT authorization_credentials_check CHECK ((expires_at > created_at)),
    CONSTRAINT authorization_credentials_token_hash_check CHECK ((octet_length(token_hash) = 32))
);

ALTER TABLE ONLY public.authorization_credentials FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_decisions (
    sequence bigint NOT NULL,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    subject text NOT NULL,
    action text NOT NULL,
    resource_kind text NOT NULL,
    resource_id text NOT NULL,
    decision jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    run_id uuid,
    remote_grant_id uuid,
    coalesced boolean DEFAULT false NOT NULL
);

ALTER TABLE ONLY public.authorization_decisions FORCE ROW LEVEL SECURITY;

--

ALTER TABLE public.authorization_decisions ALTER COLUMN sequence ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.authorization_decisions_sequence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--

CREATE TABLE public.authorization_execution (
    run_id uuid NOT NULL,
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    credential_id uuid NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT authorization_execution_subject_chain_check CHECK (((cardinality(subject_chain) >= 2) AND (cardinality(subject_chain) <= 32)))
);

ALTER TABLE ONLY public.authorization_execution FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_remote_commands (
    grant_id uuid NOT NULL,
    tenant text NOT NULL,
    request_key text NOT NULL,
    digest text NOT NULL,
    result jsonb NOT NULL
);

ALTER TABLE ONLY public.authorization_remote_commands FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_remote_execution (
    grant_id uuid NOT NULL,
    tenant text NOT NULL,
    admission_id uuid NOT NULL,
    task_id uuid NOT NULL,
    task_revision bigint NOT NULL,
    initial_task jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    human_requests jsonb DEFAULT '[]'::jsonb NOT NULL,
    activated_at timestamp with time zone
);

ALTER TABLE ONLY public.authorization_remote_execution FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_remote_outputs (
    grant_id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL
);

ALTER TABLE ONLY public.authorization_remote_outputs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_remote_terminal_receipts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    source_node text NOT NULL,
    grant_id uuid NOT NULL,
    activation jsonb NOT NULL
);

ALTER TABLE ONLY public.authorization_remote_terminal_receipts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_revisions (
    tenant text NOT NULL,
    revision bigint NOT NULL,
    document jsonb NOT NULL,
    actor text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    actor_mapping_id uuid,
    CONSTRAINT authorization_revisions_positive CHECK (COALESCE((revision > 0), false))
);

ALTER TABLE ONLY public.authorization_revisions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_run_outputs (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL
);

ALTER TABLE ONLY public.authorization_run_outputs FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_run_reads (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    resource_kind text NOT NULL,
    resource_id uuid NOT NULL,
    CONSTRAINT authorization_run_reads_resource_kind_check CHECK ((resource_kind = ANY (ARRAY['task'::text, 'artifact'::text, 'message'::text, 'run'::text, 'conversation'::text, 'generation'::text, 'workspace_events'::text])))
);

ALTER TABLE ONLY public.authorization_run_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_run_registry_reads (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL
);

ALTER TABLE ONLY public.authorization_run_registry_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_run_remote_reads (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    node_id text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    digest text NOT NULL,
    metadata jsonb NOT NULL,
    same_tenant boolean DEFAULT false NOT NULL
);

ALTER TABLE ONLY public.authorization_run_remote_reads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_task_origins (
    task_id uuid NOT NULL,
    source_run_id uuid NOT NULL,
    tenant text NOT NULL,
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL,
    CONSTRAINT authorization_task_origins_subject_chain_check CHECK (((cardinality(subject_chain) >= 2) AND (cardinality(subject_chain) <= 32)))
);

ALTER TABLE ONLY public.authorization_task_origins FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.authorization_workspaces (
    workspace_id uuid NOT NULL,
    tenant text NOT NULL,
    owner_subject text NOT NULL
);

ALTER TABLE ONLY public.authorization_workspaces FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.catalog_approval_requests (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    root_id text NOT NULL,
    root_version text NOT NULL,
    requester text NOT NULL,
    reason text NOT NULL,
    closure_digest text NOT NULL,
    state text NOT NULL,
    decline_reason text,
    decided_by text,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT catalog_approval_requests_decline CHECK ((((state = 'declined'::text) = (decline_reason IS NOT NULL)) AND ((decline_reason IS NULL) OR ((length(btrim(decline_reason)) >= 1) AND (length(btrim(decline_reason)) <= 2000))))),
    CONSTRAINT catalog_approval_requests_reason CHECK (((length(btrim(reason)) >= 1) AND (length(btrim(reason)) <= 2000))),
    CONSTRAINT catalog_approval_requests_revision CHECK ((revision > 0)),
    CONSTRAINT catalog_approval_requests_state CHECK ((state = ANY (ARRAY['open'::text, 'fulfilled'::text, 'declined'::text, 'stale'::text, 'withdrawn'::text])))
);

ALTER TABLE ONLY public.catalog_approval_requests FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.connection_scope_ceilings (
    tenant text NOT NULL,
    resource text NOT NULL,
    scopes text[] NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    revision bigint NOT NULL
);

ALTER TABLE ONLY public.connection_scope_ceilings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.credential_store_keys (
    key_id text NOT NULL,
    check_nonce bytea NOT NULL,
    check_ciphertext bytea NOT NULL,
    created_at timestamp with time zone NOT NULL
);

--

CREATE TABLE public.credential_store_resources (
    resource text NOT NULL,
    tenant text NOT NULL,
    next_version bigint NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT credential_store_counter CHECK (((next_version > 0) AND ((length(tenant) >= 1) AND (length(tenant) <= 256))))
);

ALTER TABLE ONLY public.credential_store_resources FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.credential_store_versions (
    resource text NOT NULL,
    version bigint NOT NULL,
    tenant text NOT NULL,
    key_id text NOT NULL,
    algorithm text NOT NULL,
    nonce bytea NOT NULL,
    ciphertext bytea NOT NULL,
    state text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    disabled_at timestamp with time zone,
    CONSTRAINT credential_store_version_state CHECK (((version > 0) AND (state = ANY (ARRAY['enabled'::text, 'disabled'::text])) AND ((state <> 'disabled'::text) OR (disabled_at IS NOT NULL)) AND (octet_length(nonce) = 12)))
);

ALTER TABLE ONLY public.credential_store_versions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.dashboard_administration_history (
    action text NOT NULL,
    actor_identity_id uuid,
    actor_kind text NOT NULL,
    actor_mapping_id uuid,
    id uuid NOT NULL,
    identity_id uuid,
    mapping_id uuid,
    occurred_at timestamp with time zone NOT NULL,
    registration_id uuid,
    subject text,
    tenant text,
    invitation_id uuid,
    node text,
    CONSTRAINT dashboard_administration_history_node_tenant CHECK (((node IS NULL) OR (tenant IS NOT NULL)))
);

ALTER TABLE ONLY public.dashboard_administration_history FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.dashboard_deployment_administrator_grants (
    identity_id uuid NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL
);

--

CREATE TABLE public.dashboard_execution_origins (
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    identity_id uuid NOT NULL,
    mapping_id uuid NOT NULL,
    account_id uuid,
    suspension_epoch bigint,
    source_disable_epoch bigint
);

ALTER TABLE ONLY public.dashboard_execution_origins FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.dashboard_identities (
    id uuid NOT NULL,
    issuer text NOT NULL,
    subject text NOT NULL,
    last_valid_at timestamp with time zone,
    disabled_at timestamp with time zone,
    valid_since timestamp with time zone,
    verified_email text,
    display_name text,
    display_erased_at timestamp with time zone,
    display_erased_by text,
    display_observed_at timestamp with time zone
);

--

CREATE TABLE public.dashboard_login_transactions (
    state_hash bytea NOT NULL,
    browser_hash bytea NOT NULL,
    nonce text NOT NULL,
    pkce_verifier text NOT NULL,
    return_to text NOT NULL,
    callback_uri text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    started_at timestamp with time zone,
    issuer text
);

--

CREATE TABLE public.dashboard_logout_tokens (
    jti_hash bytea NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

--

CREATE TABLE public.dashboard_mappings (
    id uuid NOT NULL,
    identity_id uuid NOT NULL,
    tenant text NOT NULL,
    subject text NOT NULL,
    credential_id uuid NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL
);

ALTER TABLE ONLY public.dashboard_mappings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.dashboard_registration_requests (
    id uuid NOT NULL,
    identity_id uuid NOT NULL,
    status text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    decided_at timestamp with time zone,
    decided_by uuid,
    decision_actor text,
    tenant text,
    invitation_id uuid
);

--

CREATE TABLE public.dashboard_sessions (
    id uuid NOT NULL,
    token_hash bytea NOT NULL,
    csrf_hash bytea NOT NULL,
    identity_id uuid NOT NULL,
    provider_sid text,
    created_at timestamp with time zone NOT NULL,
    last_activity_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    desktop boolean DEFAULT false NOT NULL,
    desktop_idle_seconds bigint,
    access_expires_at timestamp with time zone,
    auth_time timestamp with time zone
);

--

CREATE TABLE public.demo_data_loads (
    digest text NOT NULL,
    loaded_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT demo_data_loads_digest CHECK ((digest ~ '^[0-9a-f]{64}$'::text))
);

--

CREATE TABLE public.demo_reservations (
    id uuid NOT NULL,
    tenant text NOT NULL,
    email text NOT NULL,
    subject text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    spent_at timestamp with time zone,
    spent_by uuid,
    CONSTRAINT demo_reservations_state CHECK (((email = lower(email)) AND (email <> ''::text) AND (subject <> ''::text) AND ((spent_at IS NULL) = (spent_by IS NULL))))
);

ALTER TABLE ONLY public.demo_reservations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.demo_tenants (
    key text NOT NULL,
    tenant text NOT NULL,
    CONSTRAINT demo_tenants_key CHECK ((key ~ '^[a-z][a-z0-9-]{0,31}$'::text))
);

--

CREATE TABLE public.desktop_handoffs (
    id uuid NOT NULL,
    state text NOT NULL,
    challenge text NOT NULL,
    redirect_uri text NOT NULL,
    origin text NOT NULL,
    browser_session_id uuid,
    code_hash bytea,
    expires_at timestamp with time zone NOT NULL
);

--

CREATE TABLE public.desktop_refresh_credentials (
    token_hash bytea NOT NULL,
    session_id uuid NOT NULL,
    next_hash bytea
);

--

CREATE TABLE public.identity_approval_preferences (
    tenant text NOT NULL,
    subject text NOT NULL,
    default_mode text DEFAULT 'manual'::text NOT NULL,
    answer_deadline_seconds bigint,
    bypass_accepted_at timestamp with time zone,
    revision bigint NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT identity_approval_preferences_values CHECK (((default_mode = ANY (ARRAY['manual'::text, 'auto'::text, 'dont_ask'::text, 'bypass'::text])) AND ((default_mode <> 'bypass'::text) OR (bypass_accepted_at IS NOT NULL)) AND ((answer_deadline_seconds IS NULL) OR (answer_deadline_seconds > 0)) AND (revision > 0)))
);

ALTER TABLE ONLY public.identity_approval_preferences FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.identity_approval_rules (
    tenant text NOT NULL,
    id uuid NOT NULL,
    subject text NOT NULL,
    kind text NOT NULL,
    target jsonb NOT NULL,
    effect text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT identity_approval_rules_values CHECK (((kind = ANY (ARRAY['tool_call'::text, 'outbound'::text, 'web_search'::text, 'page_reading'::text, 'generation'::text])) AND (effect = ANY (ARRAY['allow'::text, 'ask'::text, 'deny'::text])) AND (jsonb_typeof(target) = ANY (ARRAY['string'::text, 'object'::text]))))
);

ALTER TABLE ONLY public.identity_approval_rules FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.identity_tenant_approval_settings (
    tenant text NOT NULL,
    decider_id text,
    decider_version text,
    revision bigint NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT identity_tenant_approval_settings_values CHECK ((((decider_id IS NULL) = (decider_version IS NULL)) AND (revision > 0)))
);

ALTER TABLE ONLY public.identity_tenant_approval_settings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.identity_thread_approval_modes (
    tenant text NOT NULL,
    thread_id uuid NOT NULL,
    subject text NOT NULL,
    mode text NOT NULL,
    revision bigint NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT identity_thread_approval_modes_values CHECK (((mode = ANY (ARRAY['manual'::text, 'auto'::text, 'dont_ask'::text, 'bypass'::text])) AND (revision > 0)))
);

ALTER TABLE ONLY public.identity_thread_approval_modes FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.node_deletions (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    deleted_at timestamp with time zone NOT NULL,
    deleted_by_kind text NOT NULL,
    deleted_by_identity uuid,
    purge_after timestamp with time zone NOT NULL,
    restored_at timestamp with time zone,
    restored_by_kind text,
    restored_by_identity uuid,
    purged_at timestamp with time zone,
    purge_step text,
    purge_failures bigint NOT NULL,
    purge_error text,
    purge_failed_at timestamp with time zone,
    purge_lease uuid,
    purge_leased_until timestamp with time zone,
    audit_erased_at timestamp with time zone,
    CONSTRAINT node_deletions_state CHECK (((deleted_by_kind = ANY (ARRAY['tenant_administrator'::text, 'deployment_administrator'::text])) AND ((deleted_by_kind <> 'tenant_administrator'::text) OR (deleted_by_identity IS NOT NULL)) AND (purge_after >= (deleted_at + '24:00:00'::interval)) AND (purge_after <= (deleted_at + '2160:00:00'::interval)) AND ((restored_at IS NULL) = (restored_by_kind IS NULL)) AND ((restored_by_kind IS NULL) OR (restored_by_kind = ANY (ARRAY['tenant_administrator'::text, 'deployment_administrator'::text]))) AND ((restored_by_kind IS DISTINCT FROM 'tenant_administrator'::text) OR (restored_by_identity IS NOT NULL)) AND ((restored_at IS NULL) OR (restored_at >= deleted_at)) AND ((restored_at IS NULL) OR (purged_at IS NULL)) AND ((purge_step IS NULL) OR (purge_step = ANY (ARRAY['capability_objects'::text, 'rows'::text, 'finish'::text]))) AND (purge_failures >= 0) AND ((purged_at IS NULL) OR (purge_step IS NULL)) AND ((purge_failures = 0) OR (purge_error IS NOT NULL)) AND ((audit_erased_at IS NULL) OR (purged_at IS NOT NULL)) AND ((purge_lease IS NULL) = (purge_leased_until IS NULL))))
);

ALTER TABLE ONLY public.node_deletions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.node_names (
    name text NOT NULL,
    tenant text NOT NULL,
    created_at timestamp with time zone NOT NULL
);

--

CREATE TABLE public.nodes (
    tenant text NOT NULL,
    name text NOT NULL,
    label text NOT NULL,
    is_default boolean NOT NULL,
    created_at timestamp with time zone NOT NULL,
    created_by_kind text NOT NULL,
    created_by_identity uuid,
    CONSTRAINT nodes_creator CHECK (((created_by_kind = ANY (ARRAY['identity'::text, 'deployment_administrator'::text])) AND ((created_by_kind <> 'identity'::text) OR (created_by_identity IS NOT NULL)) AND ((length(label) >= 1) AND (length(label) <= 64))))
);

ALTER TABLE ONLY public.nodes FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.oauth_authorizations (
    state_hash text NOT NULL,
    browser_hash text NOT NULL,
    session_id uuid NOT NULL,
    context text NOT NULL,
    tenant text NOT NULL,
    subject text NOT NULL,
    resource text NOT NULL,
    issuer text NOT NULL,
    client jsonb NOT NULL,
    scopes text[] NOT NULL,
    pkce_verifier text NOT NULL,
    redirect_uri text NOT NULL,
    return_to text NOT NULL,
    connection uuid NOT NULL,
    revision bigint NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.oauth_authorizations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.oauth_clients (
    id uuid NOT NULL,
    tenant text NOT NULL,
    owner text,
    issuer text NOT NULL,
    client_id text NOT NULL,
    secret_resource text,
    pinned_version text,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT oauth_client_secret_pin CHECK ((((pinned_version IS NULL) OR (secret_resource IS NOT NULL)) AND (revision >= 0) AND ((owner IS NULL) OR (length(owner) > 0))))
);

ALTER TABLE ONLY public.oauth_clients FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.oauth_connections (
    id uuid NOT NULL,
    tenant text NOT NULL,
    subject text NOT NULL,
    resource text NOT NULL,
    issuer text NOT NULL,
    client jsonb NOT NULL,
    scopes text[] NOT NULL,
    secret_resource text NOT NULL,
    pinned_version text NOT NULL,
    state text NOT NULL,
    access_expires_at timestamp with time zone,
    refresh_expires_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    revision bigint NOT NULL,
    CONSTRAINT oauth_connections_state CHECK (((state = ANY (ARRAY['pending'::text, 'active'::text, 'needs_reconnection'::text])) AND (revision > 0) AND ((state <> 'active'::text) OR (pinned_version <> ''::text))))
);

ALTER TABLE ONLY public.oauth_connections FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.oauth_dynamic_clients (
    id uuid NOT NULL,
    tenant text NOT NULL,
    issuer text NOT NULL,
    redirect_uri text NOT NULL,
    client_id text NOT NULL,
    secret_resource text NOT NULL,
    pinned_version text NOT NULL
);

ALTER TABLE ONLY public.oauth_dynamic_clients FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.personal_tenants (
    identity_id uuid NOT NULL,
    tenant text NOT NULL,
    created_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.personal_tenants FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.plan_authorizations (
    context text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    nonce text NOT NULL,
    pkce_verifier text NOT NULL,
    provider text NOT NULL,
    redirect_uri text NOT NULL,
    saved_client text,
    session_id uuid NOT NULL,
    state_hash text NOT NULL,
    subject text NOT NULL,
    tenant text NOT NULL,
    CONSTRAINT plan_authorization_identity CHECK (((provider = 'chatgpt'::text) AND (state_hash ~ '^[0-9a-f]{64}$'::text) AND (redirect_uri ~ '^http://127\.0\.0\.1:[0-9]{1,5}/auth/callback$'::text)))
);

--

CREATE TABLE public.plan_connection_hosts (
    created_at timestamp with time zone NOT NULL,
    host_id uuid NOT NULL,
    id text NOT NULL,
    CONSTRAINT plan_connection_host_singleton CHECK ((id = 'deployment'::text))
);

--

CREATE TABLE public.plan_connections (
    access_expires_at timestamp with time zone NOT NULL,
    account text NOT NULL,
    client_id text NOT NULL,
    connected_at timestamp with time zone NOT NULL,
    earliest_refresh_at timestamp with time zone,
    id uuid NOT NULL,
    pinned_version text,
    provider text NOT NULL,
    refresh_expires_at timestamp with time zone NOT NULL,
    refreshed_at timestamp with time zone,
    revision bigint NOT NULL,
    secret_resource text NOT NULL,
    state text NOT NULL,
    subject text NOT NULL,
    tenant text NOT NULL,
    identity_id uuid NOT NULL,
    CONSTRAINT plan_connection_catalog CHECK ((provider = 'chatgpt'::text)),
    CONSTRAINT plan_connection_metadata CHECK (((revision > 0) AND ((id)::text ~ '^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'::text) AND (secret_resource = ('aidash-store/plan-connections/'::text || (id)::text)) AND ((length(tenant) >= 1) AND (length(tenant) <= 256)) AND ((length(subject) >= 1) AND (length(subject) <= 256)) AND ((length(client_id) >= 1) AND (length(client_id) <= 256)) AND ((length(account) >= 1) AND (length(account) <= 512)))),
    CONSTRAINT plan_connection_pin CHECK (((pinned_version IS NULL) OR ((pinned_version ~ '/versions/[1-9][0-9]*$'::text) AND ("left"(pinned_version, (length(secret_resource) + 10)) = (secret_resource || '/versions/'::text))))),
    CONSTRAINT plan_connection_state CHECK (((state = ANY (ARRAY['active'::text, 'needs_reconnection'::text, 'ineligible'::text])) AND ((state <> 'active'::text) OR (pinned_version IS NOT NULL))))
);

ALTER TABLE ONLY public.plan_connections FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.provider_authorizations (
    browser_hash text NOT NULL,
    callback_uri text NOT NULL,
    context text NOT NULL,
    expected_revision bigint,
    expires_at timestamp with time zone NOT NULL,
    pkce_verifier text NOT NULL,
    provider text NOT NULL,
    provider_credential_id uuid NOT NULL,
    return_to text NOT NULL,
    session_id uuid NOT NULL,
    state_hash text NOT NULL,
    tenant text NOT NULL
);

--

CREATE TABLE public.provider_credential_bindings (
    id text NOT NULL,
    provider text NOT NULL,
    provider_credential_id uuid,
    revision bigint NOT NULL,
    tenant text NOT NULL,
    CONSTRAINT provider_credential_binding_identity CHECK (((id = ((tenant || ':'::text) || provider)) AND (provider = 'openrouter'::text) AND (revision > 0)))
);

ALTER TABLE ONLY public.provider_credential_bindings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.provider_credentials (
    base_url text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    fingerprint text NOT NULL,
    id uuid NOT NULL,
    last4 text NOT NULL,
    pinned_version text,
    provider text NOT NULL,
    revision bigint NOT NULL,
    revoked_at timestamp with time zone,
    rotated_at timestamp with time zone,
    secret_resource text NOT NULL,
    state text NOT NULL,
    tenant text NOT NULL,
    key_digest text,
    CONSTRAINT provider_credential_catalog CHECK (((provider = 'openrouter'::text) AND (base_url = 'https://openrouter.ai/api/v1'::text))),
    CONSTRAINT provider_credential_metadata CHECK (((revision > 0) AND ((id)::text ~ '^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'::text) AND (fingerprint ~ '^[0-9a-f]{16}$'::text) AND (length(last4) = 4) AND ((length(tenant) >= 1) AND (length(tenant) <= 256)))),
    CONSTRAINT provider_credential_pin CHECK (((pinned_version IS NULL) OR ((pinned_version ~ '/versions/[1-9][0-9]*$'::text) AND ("left"(pinned_version, (length(secret_resource) + 10)) = (secret_resource || '/versions/'::text))))),
    CONSTRAINT provider_credential_state CHECK (((state = ANY (ARRAY['pending'::text, 'active'::text, 'revoked'::text, 'deleted'::text])) AND ((state <> 'active'::text) OR (pinned_version IS NOT NULL)) AND ((state <> 'pending'::text) OR (pinned_version IS NULL)) AND ((state <> 'revoked'::text) OR (revoked_at IS NOT NULL))))
);

ALTER TABLE ONLY public.provider_credentials FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.remote_connection_calls (
    id uuid NOT NULL,
    tenant text NOT NULL,
    grant_id uuid NOT NULL,
    admission_id uuid NOT NULL,
    idempotency_key text NOT NULL,
    tool_key jsonb NOT NULL,
    resource text NOT NULL,
    arguments_hash text NOT NULL,
    connection_id uuid,
    secret_id uuid,
    secret_version text,
    secret_host text,
    status text NOT NULL,
    result jsonb,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT remote_connection_calls_status CHECK (((status = ANY (ARRAY['started'::text, 'completed'::text, 'failed'::text])) AND (((status = 'started'::text) AND (result IS NULL)) OR ((status <> 'started'::text) AND (result IS NOT NULL)))))
);

ALTER TABLE ONLY public.remote_connection_calls FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.run_plan_connections (
    id text NOT NULL,
    plan_connection_id uuid NOT NULL,
    provider text NOT NULL,
    run_id uuid NOT NULL,
    subject text NOT NULL,
    tenant text NOT NULL,
    identity_id uuid NOT NULL,
    CONSTRAINT run_plan_connection_identity CHECK (((id = (((run_id)::text || ':'::text) || provider)) AND (provider = 'chatgpt'::text)))
);

ALTER TABLE ONLY public.run_plan_connections FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.run_provider_credentials (
    id text NOT NULL,
    provider text NOT NULL,
    provider_credential_id uuid NOT NULL,
    run_id uuid NOT NULL,
    tenant text NOT NULL,
    CONSTRAINT run_provider_credential_identity CHECK (((id = (((run_id)::text || ':'::text) || provider)) AND (provider = 'openrouter'::text)))
);

ALTER TABLE ONLY public.run_provider_credentials FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tenant_data_protection (
    tenant text NOT NULL,
    level text NOT NULL,
    model_call_zdr boolean NOT NULL,
    plan_connection_exempt boolean NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    updated_by text NOT NULL,
    CONSTRAINT tenant_data_protection_valid CHECK (((level = ANY (ARRAY['standard'::text, 'no_training'::text, 'zero_data_retention'::text])) AND ((level <> 'zero_data_retention'::text) OR model_call_zdr)))
);

ALTER TABLE ONLY public.tenant_data_protection FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tenant_data_protection_history (
    id uuid NOT NULL,
    tenant text NOT NULL,
    level text NOT NULL,
    model_call_zdr boolean NOT NULL,
    plan_connection_exempt boolean NOT NULL,
    changed_at timestamp with time zone NOT NULL,
    changed_by text NOT NULL,
    CONSTRAINT tenant_data_protection_history_valid CHECK (((level = ANY (ARRAY['standard'::text, 'no_training'::text, 'zero_data_retention'::text])) AND ((level <> 'zero_data_retention'::text) OR model_call_zdr)))
);

ALTER TABLE ONLY public.tenant_data_protection_history FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tenant_deletions (
    id uuid NOT NULL,
    tenant text NOT NULL,
    deleted_at timestamp with time zone NOT NULL,
    deleted_by_kind text NOT NULL,
    deleted_by_identity uuid,
    purge_after timestamp with time zone NOT NULL,
    restored_at timestamp with time zone,
    restored_by_kind text,
    restored_by_identity uuid,
    purged_at timestamp with time zone,
    purge_step text,
    purge_failures bigint NOT NULL,
    purge_error text,
    purge_failed_at timestamp with time zone,
    purge_lease uuid,
    purge_leased_until timestamp with time zone,
    audit_erased_at timestamp with time zone,
    CONSTRAINT tenant_deletions_state CHECK (((deleted_by_kind = ANY (ARRAY['tenant_administrator'::text, 'deployment_administrator'::text])) AND ((deleted_by_kind <> 'tenant_administrator'::text) OR (deleted_by_identity IS NOT NULL)) AND (purge_after > deleted_at) AND ((restored_at IS NULL) = (restored_by_kind IS NULL)) AND ((restored_by_kind IS NULL) OR (restored_by_kind = ANY (ARRAY['tenant_administrator'::text, 'deployment_administrator'::text]))) AND ((restored_by_kind IS DISTINCT FROM 'tenant_administrator'::text) OR (restored_by_identity IS NOT NULL)) AND ((restored_at IS NULL) OR (restored_at >= deleted_at)) AND ((restored_at IS NULL) OR (purged_at IS NULL)) AND ((purge_step IS NULL) OR (purge_step = ANY (ARRAY['provider_credentials'::text, 'capability_objects'::text, 'rows'::text, 'finish'::text]))) AND (purge_failures >= 0) AND ((purged_at IS NULL) OR (purge_step IS NULL)) AND ((purge_failures = 0) OR (purge_error IS NOT NULL)) AND ((audit_erased_at IS NULL) OR (purged_at IS NOT NULL)) AND ((purge_lease IS NULL) = (purge_leased_until IS NULL))))
);

--

CREATE TABLE public.tenant_invitations (
    id uuid NOT NULL,
    tenant text NOT NULL,
    token_hash bytea NOT NULL,
    groups jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    issued_by_kind text NOT NULL,
    issued_by_identity uuid,
    revoked_at timestamp with time zone,
    redeemed_at timestamp with time zone,
    redeemed_by uuid,
    registration_id uuid,
    CONSTRAINT tenant_invitations_state CHECK (((octet_length(token_hash) = 32) AND (jsonb_typeof(groups) = 'array'::text) AND (expires_at > created_at) AND (issued_by_kind = ANY (ARRAY['deployment_administrator'::text, 'tenant_administrator'::text])) AND ((redeemed_at IS NULL) = (redeemed_by IS NULL))))
);

ALTER TABLE ONLY public.tenant_invitations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tenant_retention_rule_history (
    id uuid NOT NULL,
    tenant text NOT NULL,
    snapshot_after_run_seconds integer,
    snapshot_max_age_seconds integer,
    changed_at timestamp with time zone NOT NULL,
    changed_by text NOT NULL,
    CONSTRAINT tenant_retention_rule_history_periods CHECK ((((snapshot_after_run_seconds IS NULL) = (snapshot_max_age_seconds IS NULL)) AND ((snapshot_after_run_seconds IS NULL) OR (((snapshot_after_run_seconds >= 0) AND (snapshot_after_run_seconds <= 315360000)) AND ((snapshot_max_age_seconds >= 0) AND (snapshot_max_age_seconds <= 315360000))))))
);

ALTER TABLE ONLY public.tenant_retention_rule_history FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tenant_retention_rules (
    tenant text NOT NULL,
    snapshot_after_run_seconds integer NOT NULL,
    snapshot_max_age_seconds integer NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    updated_by text NOT NULL,
    CONSTRAINT tenant_retention_rules_periods CHECK ((((snapshot_after_run_seconds >= 0) AND (snapshot_after_run_seconds <= 315360000)) AND ((snapshot_max_age_seconds >= 0) AND (snapshot_max_age_seconds <= 315360000))))
);

ALTER TABLE ONLY public.tenant_retention_rules FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tenant_secrets (
    created_at timestamp with time zone NOT NULL,
    fingerprint text NOT NULL,
    hosts text[] NOT NULL,
    id uuid NOT NULL,
    last4 text NOT NULL,
    name text NOT NULL,
    pinned_version text,
    revision bigint NOT NULL,
    secret_resource text NOT NULL,
    state text NOT NULL,
    tenant text NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    owner text,
    CONSTRAINT tenant_secrets_metadata CHECK (((name ~ '^[a-z0-9][a-z0-9_-]{0,63}$'::text) AND ((cardinality(hosts) >= 1) AND (cardinality(hosts) <= 16)) AND ((id)::text ~ '^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'::text) AND (fingerprint ~ '^[0-9a-f]{16}$'::text) AND ((length(last4) >= 1) AND (length(last4) <= 4)) AND (secret_resource = ('aidash-store/tenant-secrets/'::text || (id)::text)))),
    CONSTRAINT tenant_secrets_pin CHECK (((pinned_version IS NULL) OR ((pinned_version ~ '/versions/[1-9][0-9]*$'::text) AND ("left"(pinned_version, (length(secret_resource) + 10)) = (secret_resource || '/versions/'::text))))),
    CONSTRAINT tenant_secrets_state CHECK (((state = ANY (ARRAY['pending'::text, 'active'::text, 'deleted'::text])) AND ((state <> 'active'::text) OR (pinned_version IS NOT NULL)) AND ((state <> 'pending'::text) OR (pinned_version IS NULL)) AND (revision > 0)))
);

ALTER TABLE ONLY public.tenant_secrets FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tenants (
    name text NOT NULL,
    label text,
    personal boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone NOT NULL,
    created_by_kind text NOT NULL,
    created_by_identity uuid,
    CONSTRAINT tenants_root CHECK ((((length(name) >= 1) AND (length(name) <= 256)) AND (created_by_kind = ANY (ARRAY['identity'::text, 'deployment_administrator'::text])) AND ((created_by_kind <> 'identity'::text) OR (created_by_identity IS NOT NULL)) AND ((label IS NULL) OR ((length(label) >= 1) AND (length(label) <= 256)))))
);

--

ALTER TABLE ONLY public.authorization_bundles
    ADD CONSTRAINT authorization_bundles_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.authorization_catalog_history
    ADD CONSTRAINT authorization_catalog_history_pkey PRIMARY KEY (tenant, node, entry_id, entry_version, revision);

--

ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_pkey PRIMARY KEY (tenant, node, entry_id, entry_version);

--

ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_token_hash_key UNIQUE (token_hash);

--

ALTER TABLE ONLY public.authorization_decisions
    ADD CONSTRAINT authorization_decisions_pkey PRIMARY KEY (sequence);

--

ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_pkey PRIMARY KEY (run_id);

--

ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_task_id_key UNIQUE (task_id);

--

ALTER TABLE ONLY public.authorization_remote_commands
    ADD CONSTRAINT authorization_remote_commands_pkey PRIMARY KEY (grant_id, request_key);

--

ALTER TABLE ONLY public.authorization_remote_execution
    ADD CONSTRAINT authorization_remote_execution_admission_id_key UNIQUE (admission_id);

--

ALTER TABLE ONLY public.authorization_remote_execution
    ADD CONSTRAINT authorization_remote_execution_pkey PRIMARY KEY (grant_id);

--

ALTER TABLE ONLY public.authorization_remote_execution
    ADD CONSTRAINT authorization_remote_execution_task_id_key UNIQUE (task_id);

--

ALTER TABLE ONLY public.authorization_remote_outputs
    ADD CONSTRAINT authorization_remote_outputs_pkey PRIMARY KEY (grant_id, workspace_id, resource_kind, resource_id);

--

ALTER TABLE ONLY public.authorization_remote_terminal_receipts
    ADD CONSTRAINT authorization_remote_terminal_receipts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.authorization_revisions
    ADD CONSTRAINT authorization_revisions_pkey PRIMARY KEY (tenant, revision);

--

ALTER TABLE ONLY public.authorization_run_outputs
    ADD CONSTRAINT authorization_run_outputs_pkey PRIMARY KEY (run_id, resource_kind, resource_id);

--

ALTER TABLE ONLY public.authorization_run_reads
    ADD CONSTRAINT authorization_run_reads_pkey PRIMARY KEY (run_id, resource_kind, resource_id);

--

ALTER TABLE ONLY public.authorization_run_registry_reads
    ADD CONSTRAINT authorization_run_registry_reads_pkey PRIMARY KEY (run_id, entry_id, entry_version);

--

ALTER TABLE ONLY public.authorization_run_remote_reads
    ADD CONSTRAINT authorization_run_remote_reads_pkey PRIMARY KEY (run_id, node_id, entry_id, entry_version, digest);

--

ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_pkey PRIMARY KEY (task_id);

--

ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_pkey PRIMARY KEY (workspace_id);

--

ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_tenant_workspace_id_key UNIQUE (tenant, workspace_id);

--

ALTER TABLE ONLY public.catalog_approval_requests
    ADD CONSTRAINT catalog_approval_requests_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.connection_scope_ceilings
    ADD CONSTRAINT connection_scope_ceilings_pkey PRIMARY KEY (tenant, resource);

--

ALTER TABLE ONLY public.credential_store_keys
    ADD CONSTRAINT credential_store_keys_pkey PRIMARY KEY (key_id);

--

ALTER TABLE ONLY public.credential_store_resources
    ADD CONSTRAINT credential_store_resources_pkey PRIMARY KEY (resource);

--

ALTER TABLE ONLY public.credential_store_resources
    ADD CONSTRAINT credential_store_resources_tenant_resource_key UNIQUE (tenant, resource);

--

ALTER TABLE ONLY public.credential_store_versions
    ADD CONSTRAINT credential_store_versions_pkey PRIMARY KEY (resource, version);

--

ALTER TABLE ONLY public.dashboard_administration_history
    ADD CONSTRAINT dashboard_administration_history_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.dashboard_deployment_administrator_grants
    ADD CONSTRAINT dashboard_deployment_administrator_grants_pkey PRIMARY KEY (identity_id);

--

ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_pkey PRIMARY KEY (run_id);

--

ALTER TABLE ONLY public.dashboard_identities
    ADD CONSTRAINT dashboard_identities_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.dashboard_identities
    ADD CONSTRAINT dashboard_identity_key UNIQUE (issuer, subject);

--

ALTER TABLE ONLY public.dashboard_login_transactions
    ADD CONSTRAINT dashboard_login_transactions_pkey PRIMARY KEY (state_hash);

--

ALTER TABLE ONLY public.dashboard_logout_tokens
    ADD CONSTRAINT dashboard_logout_tokens_pkey PRIMARY KEY (jti_hash);

--

ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mapping_key UNIQUE (identity_id, tenant, subject);

--

ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_credential_id_key UNIQUE (credential_id);

--

ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.dashboard_registration_requests
    ADD CONSTRAINT dashboard_registration_requests_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.dashboard_sessions
    ADD CONSTRAINT dashboard_sessions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.dashboard_sessions
    ADD CONSTRAINT dashboard_sessions_token_hash_key UNIQUE (token_hash);

--

ALTER TABLE ONLY public.demo_data_loads
    ADD CONSTRAINT demo_data_loads_pkey PRIMARY KEY (digest);

--

ALTER TABLE ONLY public.demo_reservations
    ADD CONSTRAINT demo_reservations_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.demo_reservations
    ADD CONSTRAINT demo_reservations_tenant_email_key UNIQUE (tenant, email);

--

ALTER TABLE ONLY public.demo_tenants
    ADD CONSTRAINT demo_tenants_pkey PRIMARY KEY (key);

--

ALTER TABLE ONLY public.demo_tenants
    ADD CONSTRAINT demo_tenants_tenant_key UNIQUE (tenant);

--

ALTER TABLE ONLY public.desktop_handoffs
    ADD CONSTRAINT desktop_handoffs_code_hash_key UNIQUE (code_hash);

--

ALTER TABLE ONLY public.desktop_handoffs
    ADD CONSTRAINT desktop_handoffs_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.desktop_refresh_credentials
    ADD CONSTRAINT desktop_refresh_credentials_pkey PRIMARY KEY (token_hash);

--

ALTER TABLE ONLY public.identity_approval_preferences
    ADD CONSTRAINT identity_approval_preferences_pkey PRIMARY KEY (tenant, subject);

--

ALTER TABLE ONLY public.identity_approval_rules
    ADD CONSTRAINT identity_approval_rules_pkey PRIMARY KEY (tenant, id);

--

ALTER TABLE ONLY public.identity_tenant_approval_settings
    ADD CONSTRAINT identity_tenant_approval_settings_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.identity_thread_approval_modes
    ADD CONSTRAINT identity_thread_approval_modes_pkey PRIMARY KEY (tenant, thread_id, subject);

--

ALTER TABLE ONLY public.node_deletions
    ADD CONSTRAINT node_deletions_cancellation_owner UNIQUE (id, tenant, node);

--

ALTER TABLE ONLY public.node_deletions
    ADD CONSTRAINT node_deletions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.node_names
    ADD CONSTRAINT node_names_name_tenant_key UNIQUE (name, tenant);

--

ALTER TABLE ONLY public.node_names
    ADD CONSTRAINT node_names_pkey PRIMARY KEY (name);

--

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_pkey PRIMARY KEY (tenant, name);

--

ALTER TABLE ONLY public.oauth_authorizations
    ADD CONSTRAINT oauth_authorizations_pkey PRIMARY KEY (state_hash);

--

ALTER TABLE ONLY public.oauth_clients
    ADD CONSTRAINT oauth_clients_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.oauth_connections
    ADD CONSTRAINT oauth_connections_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.oauth_dynamic_clients
    ADD CONSTRAINT oauth_dynamic_clients_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.personal_tenants
    ADD CONSTRAINT personal_tenants_pkey PRIMARY KEY (identity_id);

--

ALTER TABLE ONLY public.personal_tenants
    ADD CONSTRAINT personal_tenants_tenant_key UNIQUE (tenant);

--

ALTER TABLE ONLY public.plan_authorizations
    ADD CONSTRAINT plan_authorizations_pkey PRIMARY KEY (state_hash);

--

ALTER TABLE ONLY public.plan_connection_hosts
    ADD CONSTRAINT plan_connection_hosts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.plan_connections
    ADD CONSTRAINT plan_connection_owner UNIQUE (tenant, subject, provider);

--

ALTER TABLE ONLY public.plan_connections
    ADD CONSTRAINT plan_connection_owner_id UNIQUE (tenant, subject, provider, id);

--

ALTER TABLE ONLY public.plan_connections
    ADD CONSTRAINT plan_connections_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.provider_authorizations
    ADD CONSTRAINT provider_authorizations_pkey PRIMARY KEY (state_hash);

--

ALTER TABLE ONLY public.provider_credential_bindings
    ADD CONSTRAINT provider_credential_bindings_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.provider_credentials
    ADD CONSTRAINT provider_credential_tenant_provider_id UNIQUE (tenant, provider, id);

--

ALTER TABLE ONLY public.provider_credentials
    ADD CONSTRAINT provider_credentials_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.remote_connection_calls
    ADD CONSTRAINT remote_connection_calls_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.authorization_remote_terminal_receipts
    ADD CONSTRAINT remote_terminal_receipts_original_grant UNIQUE (tenant, node, source_node, grant_id);

--

ALTER TABLE ONLY public.run_plan_connections
    ADD CONSTRAINT run_plan_connections_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.run_provider_credentials
    ADD CONSTRAINT run_provider_credentials_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.tenant_data_protection_history
    ADD CONSTRAINT tenant_data_protection_history_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.tenant_data_protection
    ADD CONSTRAINT tenant_data_protection_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.tenant_deletions
    ADD CONSTRAINT tenant_deletions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.tenant_invitations
    ADD CONSTRAINT tenant_invitations_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.tenant_invitations
    ADD CONSTRAINT tenant_invitations_token_hash_key UNIQUE (token_hash);

--

ALTER TABLE ONLY public.tenant_retention_rule_history
    ADD CONSTRAINT tenant_retention_rule_history_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.tenant_retention_rules
    ADD CONSTRAINT tenant_retention_rules_pkey PRIMARY KEY (tenant);

--

ALTER TABLE ONLY public.tenant_secrets
    ADD CONSTRAINT tenant_secrets_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.tenants
    ADD CONSTRAINT tenants_pkey PRIMARY KEY (name);

--

CREATE UNIQUE INDEX authorization_decisions_coalesced_remote_grant ON public.authorization_decisions USING btree (tenant, remote_grant_id, revision, subject, action, resource_kind, resource_id) WHERE (coalesced AND (remote_grant_id IS NOT NULL));

--

CREATE UNIQUE INDEX authorization_decisions_coalesced_run ON public.authorization_decisions USING btree (tenant, run_id, revision, subject, action, resource_kind, resource_id) WHERE (coalesced AND (run_id IS NOT NULL));

--

CREATE UNIQUE INDEX catalog_approval_requests_open ON public.catalog_approval_requests USING btree (tenant, requester, root_id, root_version, node) WHERE (state = 'open'::text);

--

CREATE UNIQUE INDEX identity_approval_rules_owner ON public.identity_approval_rules USING btree (tenant, subject, kind, effect, target);

--

CREATE UNIQUE INDEX node_deletions_current ON public.node_deletions USING btree (tenant, node) WHERE (restored_at IS NULL);

--

CREATE UNIQUE INDEX nodes_one_default ON public.nodes USING btree (tenant) WHERE is_default;

--

CREATE UNIQUE INDEX oauth_clients_owner_issuer ON public.oauth_clients USING btree (tenant, COALESCE(owner, ''::text), issuer);

--

CREATE UNIQUE INDEX oauth_connections_owner_resource ON public.oauth_connections USING btree (tenant, subject, resource);

--

CREATE UNIQUE INDEX oauth_dynamic_clients_key ON public.oauth_dynamic_clients USING btree (tenant, issuer, redirect_uri);

--

CREATE UNIQUE INDEX remote_connection_calls_grant_key ON public.remote_connection_calls USING btree (grant_id, idempotency_key);

--

CREATE UNIQUE INDEX tenant_deletions_current ON public.tenant_deletions USING btree (tenant) WHERE (restored_at IS NULL);

--

CREATE UNIQUE INDEX tenant_secrets_live_name ON public.tenant_secrets USING btree (tenant, COALESCE(owner, ''::text), name) WHERE (state <> 'deleted'::text);

--

ALTER TABLE ONLY public.authorization_bundles
    ADD CONSTRAINT authorization_bundles_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_catalog_history
    ADD CONSTRAINT authorization_catalog_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_node_fkey FOREIGN KEY (tenant, node) REFERENCES public.nodes(tenant, name);

--

ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.authorization_catalog
    ADD CONSTRAINT authorization_catalog_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.authorization_credentials
    ADD CONSTRAINT authorization_credentials_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_decisions
    ADD CONSTRAINT authorization_decisions_tenant_revision_fkey FOREIGN KEY (tenant, revision) REFERENCES public.authorization_revisions(tenant, revision);

--

ALTER TABLE ONLY public.authorization_decisions
    ADD CONSTRAINT authorization_decisions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_credential_id_fkey FOREIGN KEY (tenant, credential_id) REFERENCES public.authorization_credentials(tenant, id);

--

ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.authorization_execution
    ADD CONSTRAINT authorization_execution_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_remote_commands
    ADD CONSTRAINT authorization_remote_commands_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_remote_execution
    ADD CONSTRAINT authorization_remote_execution_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_remote_outputs
    ADD CONSTRAINT authorization_remote_outputs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_revisions
    ADD CONSTRAINT authorization_revisions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_run_outputs
    ADD CONSTRAINT authorization_run_outputs_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_run_reads
    ADD CONSTRAINT authorization_run_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_run_registry_reads
    ADD CONSTRAINT authorization_run_registry_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_run_remote_reads
    ADD CONSTRAINT authorization_run_remote_reads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.authorization_task_origins
    ADD CONSTRAINT authorization_task_origins_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.authorization_bundles(tenant);

--

ALTER TABLE ONLY public.authorization_workspaces
    ADD CONSTRAINT authorization_workspaces_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.catalog_approval_requests
    ADD CONSTRAINT catalog_approval_requests_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.connection_scope_ceilings
    ADD CONSTRAINT connection_scope_ceilings_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.credential_store_versions
    ADD CONSTRAINT credential_store_resource_fk FOREIGN KEY (tenant, resource) REFERENCES public.credential_store_resources(tenant, resource) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.credential_store_resources
    ADD CONSTRAINT credential_store_resources_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.credential_store_versions
    ADD CONSTRAINT credential_store_versions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.dashboard_administration_history
    ADD CONSTRAINT dashboard_administration_history_node_owner_fkey FOREIGN KEY (node, tenant) REFERENCES public.node_names(name, tenant);

--

ALTER TABLE ONLY public.dashboard_administration_history
    ADD CONSTRAINT dashboard_administration_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.dashboard_deployment_administrator_grants
    ADD CONSTRAINT dashboard_deployment_administrator_grants_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_mapping_id_fkey FOREIGN KEY (tenant, mapping_id) REFERENCES public.dashboard_mappings(tenant, id);

--

ALTER TABLE ONLY public.dashboard_execution_origins
    ADD CONSTRAINT dashboard_execution_origins_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_credential_id_fkey FOREIGN KEY (tenant, credential_id) REFERENCES public.authorization_credentials(tenant, id);

--

ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.dashboard_mappings
    ADD CONSTRAINT dashboard_mappings_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.dashboard_registration_requests
    ADD CONSTRAINT dashboard_registration_requests_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.dashboard_registration_requests
    ADD CONSTRAINT dashboard_registration_requests_invitation_id_fkey FOREIGN KEY (invitation_id) REFERENCES public.tenant_invitations(id);

--

ALTER TABLE ONLY public.dashboard_registration_requests
    ADD CONSTRAINT dashboard_registration_requests_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.dashboard_sessions
    ADD CONSTRAINT dashboard_sessions_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.demo_reservations
    ADD CONSTRAINT demo_reservations_spent_by_fkey FOREIGN KEY (spent_by) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.demo_reservations
    ADD CONSTRAINT demo_reservations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.demo_tenants
    ADD CONSTRAINT demo_tenants_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.desktop_handoffs
    ADD CONSTRAINT desktop_handoffs_browser_session_id_fkey FOREIGN KEY (browser_session_id) REFERENCES public.dashboard_sessions(id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.desktop_refresh_credentials
    ADD CONSTRAINT desktop_refresh_credentials_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.dashboard_sessions(id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.identity_approval_preferences
    ADD CONSTRAINT identity_approval_preferences_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.identity_approval_rules
    ADD CONSTRAINT identity_approval_rules_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.identity_tenant_approval_settings
    ADD CONSTRAINT identity_tenant_approval_settings_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.identity_thread_approval_modes
    ADD CONSTRAINT identity_thread_approval_modes_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.node_deletions
    ADD CONSTRAINT node_deletions_deleted_by_identity_fkey FOREIGN KEY (deleted_by_identity) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.node_deletions
    ADD CONSTRAINT node_deletions_node_owner_fkey FOREIGN KEY (node, tenant) REFERENCES public.node_names(name, tenant);

--

ALTER TABLE ONLY public.node_deletions
    ADD CONSTRAINT node_deletions_restored_by_identity_fkey FOREIGN KEY (restored_by_identity) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.node_deletions
    ADD CONSTRAINT node_deletions_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.node_names
    ADD CONSTRAINT node_names_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_name_fkey FOREIGN KEY (name, tenant) REFERENCES public.node_names(name, tenant);

--

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.oauth_authorizations
    ADD CONSTRAINT oauth_authorizations_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.oauth_clients
    ADD CONSTRAINT oauth_clients_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.oauth_connections
    ADD CONSTRAINT oauth_connections_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.oauth_dynamic_clients
    ADD CONSTRAINT oauth_dynamic_clients_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.personal_tenants
    ADD CONSTRAINT personal_tenants_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.personal_tenants
    ADD CONSTRAINT personal_tenants_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.plan_connections
    ADD CONSTRAINT plan_connections_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.plan_connections
    ADD CONSTRAINT plan_connections_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.provider_credential_bindings
    ADD CONSTRAINT provider_credential_bindings_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.provider_credential_bindings
    ADD CONSTRAINT provider_credential_bindings_tenant_target FOREIGN KEY (tenant, provider, provider_credential_id) REFERENCES public.provider_credentials(tenant, provider, id);

--

ALTER TABLE ONLY public.provider_credentials
    ADD CONSTRAINT provider_credentials_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.remote_connection_calls
    ADD CONSTRAINT remote_connection_calls_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.authorization_remote_terminal_receipts
    ADD CONSTRAINT remote_terminal_receipts_owner_fkey FOREIGN KEY (node, tenant) REFERENCES public.node_names(name, tenant);

--

ALTER TABLE ONLY public.authorization_remote_terminal_receipts
    ADD CONSTRAINT remote_terminal_receipts_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_plan_connections
    ADD CONSTRAINT run_plan_connections_identity_id_fkey FOREIGN KEY (identity_id) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.run_plan_connections
    ADD CONSTRAINT run_plan_connections_owner_target FOREIGN KEY (tenant, subject, provider, plan_connection_id) REFERENCES public.plan_connections(tenant, subject, provider, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.run_plan_connections
    ADD CONSTRAINT run_plan_connections_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_provider_credentials
    ADD CONSTRAINT run_provider_credentials_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.run_provider_credentials
    ADD CONSTRAINT run_provider_credentials_tenant_target FOREIGN KEY (tenant, provider, provider_credential_id) REFERENCES public.provider_credentials(tenant, provider, id);

--

ALTER TABLE ONLY public.tenant_data_protection_history
    ADD CONSTRAINT tenant_data_protection_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tenant_data_protection
    ADD CONSTRAINT tenant_data_protection_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tenant_deletions
    ADD CONSTRAINT tenant_deletions_deleted_by_identity_fkey FOREIGN KEY (deleted_by_identity) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.tenant_deletions
    ADD CONSTRAINT tenant_deletions_restored_by_identity_fkey FOREIGN KEY (restored_by_identity) REFERENCES public.dashboard_identities(id);

--

ALTER TABLE ONLY public.tenant_deletions
    ADD CONSTRAINT tenant_deletions_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tenant_invitations
    ADD CONSTRAINT tenant_invitations_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tenant_retention_rule_history
    ADD CONSTRAINT tenant_retention_rule_history_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tenant_retention_rules
    ADD CONSTRAINT tenant_retention_rules_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tenant_secrets
    ADD CONSTRAINT tenant_secrets_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);

--

ALTER TABLE ONLY public.tenants
    ADD CONSTRAINT tenants_created_by_identity_fkey FOREIGN KEY (created_by_identity) REFERENCES public.dashboard_identities(id);

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

SELECT pg_catalog.setval('public.authorization_decisions_sequence_seq', 1, false);
