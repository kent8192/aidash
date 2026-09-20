ALTER TABLE registry DROP CONSTRAINT registry_kind_check;
ALTER TABLE registry ADD CONSTRAINT registry_kind_check
    CHECK (kind IN ('agent','model','tool','skill','cluster','node','compactor'));
ALTER TABLE generation_policies ADD COLUMN allocated_compaction_calls bigint NOT NULL DEFAULT 0
    CHECK (allocated_compaction_calls >= 0);
ALTER TABLE generation_budgets ADD COLUMN compaction_call_limit bigint NOT NULL DEFAULT 0
    CHECK (compaction_call_limit >= 0);
ALTER TABLE generation_budgets ADD COLUMN compaction_calls bigint NOT NULL DEFAULT 0
    CHECK (compaction_calls >= 0 AND compaction_calls <= compaction_call_limit);
CREATE TABLE generation_compaction_usage (
    request_id uuid NOT NULL REFERENCES generation_requests(id),
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL REFERENCES runs(id),
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    request_bytes bigint NOT NULL CHECK (request_bytes > 0),
    questions integer NOT NULL CHECK (questions > 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (request_id, attempt_id),
    FOREIGN KEY (provider_id, provider_version) REFERENCES registry(id, version)
);
