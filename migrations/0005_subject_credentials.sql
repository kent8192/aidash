CREATE TABLE authorization_credentials (
    id uuid PRIMARY KEY,
    tenant text NOT NULL REFERENCES authorization_bundles(tenant),
    subject text NOT NULL,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    issued_by text NOT NULL,
    CHECK (expires_at > created_at)
);
CREATE INDEX authorization_credentials_tenant ON authorization_credentials(tenant, created_at, id);

-- Ownership is authoritative and independent of caller-editable workspace state.
-- Legacy workspaces have no row and remain accessible only to the operator.
CREATE TABLE authorization_workspaces (
    workspace_id uuid PRIMARY KEY REFERENCES workspaces(id),
    tenant text NOT NULL REFERENCES authorization_bundles(tenant),
    owner_subject text NOT NULL
);
CREATE INDEX authorization_workspaces_tenant ON authorization_workspaces(tenant, workspace_id);
