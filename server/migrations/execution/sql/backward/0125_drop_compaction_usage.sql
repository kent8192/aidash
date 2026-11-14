-- Restore the composite primary key explicitly: the historical state marks
-- both columns as individual primary keys, which typed reversal cannot render.
CREATE TABLE public.generation_compaction_usage (
    request_id uuid NOT NULL,
    tenant text NOT NULL,
    attempt_id uuid NOT NULL,
    run_id uuid NOT NULL,
    provider_id text NOT NULL,
    provider_version text NOT NULL,
    request_bytes bigint NOT NULL,
    questions integer NOT NULL,
    created_at timestamptz NOT NULL,
    CONSTRAINT generation_compaction_usage_pkey PRIMARY KEY (request_id, attempt_id),
    CONSTRAINT generation_compaction_usage_run_id_fkey FOREIGN KEY (tenant, run_id)
        REFERENCES public.runs (tenant, id)
);
