-- Restores the Research tables as they stood after execution/0131, with their
-- keys, checks, indexes and forced row policies, empty. The deleted Web
-- disclosure approval records and the tables' rows are not restored.
CREATE TABLE public.research_account_terminations (
    account_id text NOT NULL,
    terminated_at timestamp with time zone NOT NULL,
    deletion_after_termination_days integer NOT NULL,
    recorded_at timestamp with time zone NOT NULL,
    CONSTRAINT research_account_terminations_period CHECK (((char_length(account_id) > 0) AND (deletion_after_termination_days > 0)))
);
CREATE TABLE public.research_attempts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    invocation_key text NOT NULL,
    operation text NOT NULL,
    provider text NOT NULL,
    destination text NOT NULL,
    account_id text,
    price_micro_usd bigint,
    outcome text NOT NULL,
    result jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    retention_account_id text,
    retention_days integer,
    purged_at timestamp with time zone,
    purge_reason text,
    CONSTRAINT research_attempts_account_terms CHECK ((((retention_account_id IS NULL) = (retention_days IS NULL)) AND ((retention_days IS NULL) OR (retention_days > 0)) AND ((purged_at IS NULL) = (purge_reason IS NULL)) AND ((purge_reason IS NULL) OR (purge_reason = 'account_terms'::text)))),
    CONSTRAINT research_attempts_attribution CHECK ((((account_id IS NULL) = (price_micro_usd IS NULL)) AND ((price_micro_usd IS NULL) OR (price_micro_usd >= 0)))),
    CONSTRAINT research_attempts_operation CHECK ((operation = ANY (ARRAY['search'::text, 'fetch'::text]))),
    CONSTRAINT research_attempts_outcome CHECK (((outcome = ANY (ARRAY['dispatched'::text, 'completed'::text, 'failed'::text, 'unknown'::text])) AND ((outcome = 'dispatched'::text) = (completed_at IS NULL)) AND ((outcome = 'dispatched'::text) = (result IS NULL))))
);
ALTER TABLE ONLY public.research_attempts FORCE ROW LEVEL SECURITY;
CREATE TABLE public.research_fragments (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    snapshot_id uuid NOT NULL,
    text text NOT NULL,
    line_start integer NOT NULL,
    line_end integer NOT NULL,
    pages jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    image_page integer,
    image_media_type text,
    image_width integer,
    image_height integer,
    image_digest text,
    passage_digest text,
    CONSTRAINT research_fragments_lines CHECK (((jsonb_typeof(pages) = 'array'::text) AND (((image_page IS NULL) AND (image_media_type IS NULL) AND (image_width IS NULL) AND (image_height IS NULL) AND (image_digest IS NULL) AND (line_start >= 1) AND (line_end >= line_start)) OR ((image_page >= 1) AND (line_start = 0) AND (line_end = 0) AND (image_media_type = 'image/png'::text) AND ((image_width >= 1) AND (image_width <= 1568)) AND ((image_height >= 1) AND (image_height <= 1568)) AND (image_digest ~~ 'sha256:%'::text)))))
);
ALTER TABLE ONLY public.research_fragments FORCE ROW LEVEL SECURITY;
CREATE TABLE public.research_journal_results (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    invocation_key text NOT NULL,
    digest text NOT NULL,
    reference jsonb NOT NULL,
    bytes bytea,
    purge_reason text,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT research_journal_results_payload CHECK ((((bytes IS NULL) OR (((octet_length(bytes) >= 1) AND (octet_length(bytes) <= 1048576)) AND (purge_reason IS NULL))) AND ((purge_reason IS NULL) OR (purge_reason = ANY (ARRAY['account_terms'::text, 'revocation'::text]))) AND (digest ~ '^sha256:[a-f0-9]{64}$'::text)))
);
ALTER TABLE ONLY public.research_journal_results FORCE ROW LEVEL SECURITY;
CREATE TABLE public.research_originals (
    snapshot_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    bytes bytea NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT research_originals_bytes CHECK (((octet_length(bytes) >= 1) AND (octet_length(bytes) <= 16777216)))
);
ALTER TABLE ONLY public.research_originals FORCE ROW LEVEL SECURITY;
CREATE TABLE public.research_page_images (
    fragment_id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    bytes bytea NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT research_page_images_bytes CHECK (((octet_length(bytes) >= 1) AND (octet_length(bytes) <= 1048576)))
);
ALTER TABLE ONLY public.research_page_images FORCE ROW LEVEL SECURITY;
CREATE TABLE public.research_revocations (
    run_id uuid NOT NULL,
    tenant text,
    revoked_by jsonb NOT NULL,
    reason text NOT NULL,
    revoked_at timestamp with time zone NOT NULL,
    removed jsonb NOT NULL,
    CONSTRAINT research_revocations_record CHECK ((((char_length(reason) >= 1) AND (char_length(reason) <= 1000)) AND (jsonb_typeof(revoked_by) = 'object'::text) AND (jsonb_typeof(removed) = 'object'::text)))
);
ALTER TABLE ONLY public.research_revocations FORCE ROW LEVEL SECURITY;
CREATE TABLE public.research_snapshots (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    source_id uuid NOT NULL,
    requested_url text NOT NULL,
    url text NOT NULL,
    title text,
    media_type text NOT NULL,
    fetched_at timestamp with time zone NOT NULL,
    raw_digest text NOT NULL,
    text_digest text NOT NULL,
    completeness text NOT NULL,
    lines jsonb NOT NULL,
    text_bytes bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    purged_at timestamp with time zone,
    purge_reason text,
    page_count integer,
    pages_without_text jsonb DEFAULT '[]'::jsonb NOT NULL,
    original_digest text,
    original_bytes bigint,
    original_retained boolean DEFAULT false NOT NULL,
    attempt_key text,
    CONSTRAINT research_snapshots_completeness CHECK ((completeness = ANY (ARRAY['complete'::text, 'text_limit'::text]))),
    CONSTRAINT research_snapshots_digests CHECK (((raw_digest ~~ 'sha256:%'::text) AND (text_digest ~~ 'sha256:%'::text) AND (text_bytes >= 0) AND (jsonb_typeof(lines) = 'array'::text))),
    CONSTRAINT research_snapshots_pages CHECK ((((page_count IS NULL) OR (page_count >= 1)) AND (jsonb_typeof(pages_without_text) = 'array'::text) AND ((original_digest IS NULL) = (original_bytes IS NULL)) AND ((original_digest IS NULL) OR (original_digest ~~ 'sha256:%'::text)) AND ((original_bytes IS NULL) OR ((original_bytes >= 1) AND (original_bytes <= 16777216))) AND ((NOT original_retained) OR ((original_digest IS NOT NULL) AND (purged_at IS NULL))))),
    CONSTRAINT research_snapshots_purge CHECK ((((purged_at IS NULL) = (purge_reason IS NULL)) AND ((purge_reason IS NULL) OR ((purge_reason = ANY (ARRAY['retention'::text, 'revocation'::text, 'account_terms'::text])) AND (text_bytes = 0) AND (lines = '[]'::jsonb)))))
);
ALTER TABLE ONLY public.research_snapshots FORCE ROW LEVEL SECURITY;
CREATE TABLE public.research_sources (
    id uuid NOT NULL,
    tenant text NOT NULL,
    run_id uuid NOT NULL,
    url text NOT NULL,
    title text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    attempt_key text,
    CONSTRAINT research_sources_bounds CHECK ((((length(url) >= 1) AND (length(url) <= 4096)) AND (length(title) <= 512)))
);
ALTER TABLE ONLY public.research_sources FORCE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.research_account_terminations
    ADD CONSTRAINT research_account_terminations_pkey PRIMARY KEY (account_id);
ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_invocation UNIQUE (run_id, invocation_key);
ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_tenant_id_key UNIQUE (tenant, id);
ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_variant UNIQUE (run_id, invocation_key, digest);
ALTER TABLE ONLY public.research_originals
    ADD CONSTRAINT research_originals_pkey PRIMARY KEY (snapshot_id);
ALTER TABLE ONLY public.research_page_images
    ADD CONSTRAINT research_page_images_pkey PRIMARY KEY (fragment_id);
ALTER TABLE ONLY public.research_revocations
    ADD CONSTRAINT research_revocations_pkey PRIMARY KEY (run_id);
ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_run_identity UNIQUE (run_id, id);
ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_tenant_run_id_id_key UNIQUE (tenant, run_id, id);
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_run_identity UNIQUE (run_id, id);
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_tenant_run_id_id_key UNIQUE (tenant, run_id, id);
CREATE INDEX idx_research_attempts_account_id_created_at_d4f5ed98697a2315 ON public.research_attempts USING btree (account_id, created_at) WHERE (account_id IS NOT NULL);
CREATE INDEX idx_research_attempts_id_c91d99759e4ae2e3 ON public.research_attempts USING btree (id) WHERE ((retention_account_id IS NOT NULL) AND (purged_at IS NULL));
CREATE INDEX idx_research_originals_run_id ON public.research_originals USING btree (run_id);
CREATE INDEX idx_research_page_images_run_id ON public.research_page_images USING btree (run_id);
CREATE INDEX idx_research_revocations_tenant_revoked_at_39caea2f02a3c4fc ON public.research_revocations USING btree (tenant, revoked_at) WHERE (tenant IS NOT NULL);
CREATE INDEX idx_research_snapshots_id_f1769031a31290d4 ON public.research_snapshots USING btree (id) WHERE (purged_at IS NULL);
CREATE INDEX idx_research_snapshots_run_id_attempt_key ON public.research_snapshots USING btree (run_id, attempt_key);
CREATE INDEX idx_research_sources_run_id_attempt_key ON public.research_sources USING btree (run_id, attempt_key);
ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;
ALTER TABLE ONLY public.research_attempts
    ADD CONSTRAINT research_attempts_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_snapshot FOREIGN KEY (tenant, run_id, snapshot_id) REFERENCES public.research_snapshots(tenant, run_id, id) ON UPDATE RESTRICT ON DELETE CASCADE;
ALTER TABLE ONLY public.research_fragments
    ADD CONSTRAINT research_fragments_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;
ALTER TABLE ONLY public.research_journal_results
    ADD CONSTRAINT research_journal_results_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.research_originals
    ADD CONSTRAINT research_originals_snapshot FOREIGN KEY (tenant, run_id, snapshot_id) REFERENCES public.research_snapshots(tenant, run_id, id) ON UPDATE RESTRICT ON DELETE CASCADE;
ALTER TABLE ONLY public.research_originals
    ADD CONSTRAINT research_originals_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.research_page_images
    ADD CONSTRAINT research_page_images_fragment FOREIGN KEY (tenant, fragment_id) REFERENCES public.research_fragments(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;
ALTER TABLE ONLY public.research_page_images
    ADD CONSTRAINT research_page_images_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.research_revocations
    ADD CONSTRAINT research_revocations_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_source FOREIGN KEY (tenant, run_id, source_id) REFERENCES public.research_sources(tenant, run_id, id) ON UPDATE RESTRICT ON DELETE CASCADE;
ALTER TABLE ONLY public.research_snapshots
    ADD CONSTRAINT research_snapshots_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_run FOREIGN KEY (tenant, run_id) REFERENCES public.runs(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;
ALTER TABLE ONLY public.research_sources
    ADD CONSTRAINT research_sources_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE public.research_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_fragments ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_journal_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_originals ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_page_images ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_revocations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_snapshots ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.research_sources ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.research_attempts USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.research_fragments USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.research_journal_results USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.research_originals USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.research_page_images USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.research_revocations USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.research_snapshots USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
CREATE POLICY tenant_isolation ON public.research_sources USING ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant)))) WITH CHECK ((( SELECT public.aidash_deployment_scope() AS aidash_deployment_scope) OR (tenant = ( SELECT public.aidash_scope_tenant() AS aidash_scope_tenant))));
