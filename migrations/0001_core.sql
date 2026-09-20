CREATE TABLE registry (
    id text NOT NULL,
    version text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('agent','model','tool','skill','cluster','node')),
    metadata jsonb NOT NULL,
    PRIMARY KEY (id, version)
);
CREATE INDEX registry_metadata ON registry USING gin (metadata);

CREATE TABLE workspaces (
    id uuid PRIMARY KEY,
    title text NOT NULL,
    goal text NOT NULL,
    state jsonb NOT NULL DEFAULT '{}',
    revision bigint NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE tasks (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    title text NOT NULL,
    description text NOT NULL,
    status text NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','CLAIMED','RUNNING','COMPLETED','FAILED','BLOCKED','CANCELLED')),
    requirements jsonb NOT NULL DEFAULT '{}',
    owner text,
    created_by text NOT NULL,
    dependencies uuid[] NOT NULL DEFAULT '{}',
    parent_id uuid REFERENCES tasks(id),
    revision bigint NOT NULL DEFAULT 0,
    creation_key text UNIQUE,
    completion_key text UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'OPEN' AND owner IS NULL) OR status <> 'OPEN')
);
CREATE INDEX tasks_workspace ON tasks(workspace_id, created_at);

-- This is both the durable event log and the transactional outbox.
CREATE TABLE events (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL UNIQUE,
    node_id text NOT NULL,
    workspace_id uuid REFERENCES workspaces(id),
    kind text NOT NULL,
    data jsonb NOT NULL,
    published_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX events_outbox ON events(sequence) WHERE published_at IS NULL;
CREATE INDEX events_workspace ON events(workspace_id, sequence);
CREATE TABLE inbox (event_id uuid PRIMARY KEY, received_at timestamptz NOT NULL DEFAULT now());

CREATE TABLE artifacts (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    task_id uuid NOT NULL REFERENCES tasks(id),
    kind text NOT NULL CHECK (kind IN ('text','json','file_reference','code','structured_result')),
    name text NOT NULL,
    content jsonb NOT NULL,
    created_by text NOT NULL,
    idempotency_key text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE conversations (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    target text NOT NULL,
    target_kind text NOT NULL CHECK (target_kind IN ('agent', 'cluster')),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE messages (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(id),
    sender text NOT NULL,
    content text NOT NULL,
    idempotency_key text UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- A remote run keeps its journal on the executing node; task ownership remains
-- on the home node. No cross-node database access or distributed transaction.
CREATE TABLE runs (
    id uuid PRIMARY KEY,
    task_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    home_node text NOT NULL,
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    phase text NOT NULL DEFAULT 'READY' CHECK (phase IN ('READY','THINKING','TOOL_CALL','WAITING','COMPLETED','FAILED','CANCELLED')),
    control text NOT NULL DEFAULT 'ACTIVE' CHECK (control IN ('ACTIVE','PAUSED','CANCELLED')),
    context jsonb NOT NULL DEFAULT '{}',
    pending jsonb NOT NULL DEFAULT '{}',
    step integer NOT NULL DEFAULT 0,
    revision bigint NOT NULL DEFAULT 0,
    error text,
    lease_owner uuid,
    lease_until timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (home_node, task_id)
);
CREATE INDEX runs_ready ON runs(updated_at) WHERE phase NOT IN ('COMPLETED','FAILED','CANCELLED');
CREATE TABLE invocations (
    idempotency_key text PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES runs(id),
    tool text NOT NULL,
    input jsonb NOT NULL,
    status text NOT NULL CHECK (status IN ('STARTED','COMPLETED','UNCERTAIN')),
    result jsonb,
    replay_safe boolean NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE human_requests (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    run_id uuid NOT NULL REFERENCES runs(id),
    kind text NOT NULL CHECK (kind IN ('QUESTION','APPROVAL_REQUIRED','CONFIRMATION','INFORMATION_REQUEST')),
    prompt text NOT NULL,
    response jsonb,
    request_key text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE memory (
    agent_id text NOT NULL,
    agent_version text NOT NULL,
    workspace_id uuid NOT NULL,
    data jsonb NOT NULL DEFAULT '{}',
    PRIMARY KEY (agent_id, agent_version, workspace_id)
);

CREATE TABLE peers (
    node_id text PRIMARY KEY,
    endpoint text NOT NULL,
    credential_env text NOT NULL,
    protocol_version text NOT NULL,
    enabled boolean NOT NULL DEFAULT true
);
CREATE TABLE packages (
    id text NOT NULL,
    version text NOT NULL,
    manifest jsonb NOT NULL,
    digest text NOT NULL,
    PRIMARY KEY (id, version)
);
CREATE TABLE installations (
    id text NOT NULL,
    version text NOT NULL,
    digest text NOT NULL,
    config jsonb NOT NULL,
    installed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (id, version),
    FOREIGN KEY (id, version) REFERENCES registry(id, version)
);
