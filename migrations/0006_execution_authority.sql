CREATE TABLE authorization_catalog (
    tenant text NOT NULL REFERENCES authorization_bundles(tenant),
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    enabled boolean NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    PRIMARY KEY (tenant, entry_id, entry_version),
    FOREIGN KEY (entry_id, entry_version) REFERENCES registry(id, version)
);
CREATE TABLE authorization_catalog_history (
    tenant text NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL,
    revision bigint NOT NULL,
    enabled boolean NOT NULL,
    actor text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, entry_id, entry_version, revision),
    FOREIGN KEY (tenant, entry_id, entry_version) REFERENCES authorization_catalog(tenant, entry_id, entry_version)
);

-- Only trusted admission code constructs these grants. Credentials and policy
-- are reloaded at every worker boundary; no bearer secret is copied into a run.
CREATE TABLE authorization_execution (
    run_id uuid PRIMARY KEY REFERENCES runs(id),
    task_id uuid NOT NULL UNIQUE REFERENCES tasks(id),
    workspace_id uuid NOT NULL REFERENCES authorization_workspaces(workspace_id),
    tenant text NOT NULL REFERENCES authorization_bundles(tenant),
    credential_id uuid NOT NULL REFERENCES authorization_credentials(id),
    root_subject text NOT NULL,
    subject_chain text[] NOT NULL CHECK (cardinality(subject_chain) BETWEEN 2 AND 32),
    created_at timestamptz NOT NULL DEFAULT now()
);
