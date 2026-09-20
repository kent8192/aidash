CREATE TABLE generation_policies (
    tenant text NOT NULL REFERENCES authorization_bundles(tenant),
    id text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    spec jsonb NOT NULL,
    generated_count bigint NOT NULL DEFAULT 0 CHECK (generated_count >= 0),
    allocated_tokens bigint NOT NULL DEFAULT 0 CHECK (allocated_tokens >= 0),
    PRIMARY KEY (tenant,id)
);
CREATE TABLE generation_policy_history (
    tenant text NOT NULL,
    policy_id text NOT NULL,
    revision bigint NOT NULL,
    spec jsonb NOT NULL,
    actor text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant,policy_id,revision),
    FOREIGN KEY (tenant,policy_id) REFERENCES generation_policies(tenant,id)
);
CREATE TABLE authorization_task_origins (
    task_id uuid PRIMARY KEY REFERENCES tasks(id),
    source_run_id uuid NOT NULL REFERENCES runs(id),
    tenant text NOT NULL REFERENCES authorization_bundles(tenant),
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL CHECK (cardinality(subject_chain) BETWEEN 2 AND 32)
);
CREATE TABLE generation_requests (
    id uuid PRIMARY KEY,
    tenant text NOT NULL,
    policy_id text NOT NULL,
    policy_revision bigint NOT NULL,
    task_id uuid NOT NULL UNIQUE REFERENCES tasks(id),
    workspace_id uuid NOT NULL REFERENCES authorization_workspaces(workspace_id),
    credential_id uuid NOT NULL REFERENCES authorization_credentials(id),
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL CHECK (cardinality(subject_chain) BETWEEN 1 AND 31),
    agent_id text NOT NULL UNIQUE,
    agent_version text NOT NULL,
    definition jsonb NOT NULL,
    status text NOT NULL CHECK (status IN ('PENDING_APPROVAL','QUEUED','ACTIVE','COMPLETED','DENIED','STOPPED','EXPIRED','FAILED','DELETED')),
    reason text NOT NULL,
    depth integer NOT NULL CHECK (depth BETWEEN 1 AND 31),
    token_limit bigint NOT NULL CHECK (token_limit > 0),
    quota_released boolean NOT NULL DEFAULT false,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant,policy_id,policy_revision) REFERENCES generation_policy_history(tenant,policy_id,revision)
);
CREATE INDEX generation_requests_policy ON generation_requests(tenant,policy_id,status);
CREATE TABLE generation_history (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id uuid NOT NULL REFERENCES generation_requests(id),
    status text NOT NULL,
    actor text NOT NULL,
    reason text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
-- Token accounting is separate from the lifecycle row so a worker can persist
-- a reservation while holding that row's authority lease across an effect.
CREATE TABLE generation_budgets (
    request_id uuid PRIMARY KEY REFERENCES generation_requests(id),
    token_limit bigint NOT NULL CHECK (token_limit > 0),
    used_tokens bigint NOT NULL DEFAULT 0 CHECK (used_tokens >= 0 AND used_tokens <= token_limit)
);
CREATE TABLE generation_usage (
    request_id uuid NOT NULL REFERENCES generation_requests(id),
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL REFERENCES runs(id),
    reserved_tokens bigint NOT NULL CHECK (reserved_tokens > 0),
    reported_tokens bigint CHECK (reported_tokens >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (request_id,attempt_id)
);
