CREATE TABLE authorization_bundles (
    tenant text PRIMARY KEY,
    revision bigint NOT NULL CHECK (revision > 0),
    document jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE authorization_revisions (
    tenant text NOT NULL REFERENCES authorization_bundles(tenant),
    revision bigint NOT NULL,
    document jsonb NOT NULL,
    actor text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, revision)
);

CREATE TABLE authorization_decisions (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant text NOT NULL,
    revision bigint NOT NULL,
    subject text NOT NULL,
    action text NOT NULL,
    resource_kind text NOT NULL,
    resource_id text NOT NULL,
    decision jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant, revision) REFERENCES authorization_revisions(tenant, revision)
);
CREATE INDEX authorization_decisions_tenant ON authorization_decisions(tenant, sequence);
