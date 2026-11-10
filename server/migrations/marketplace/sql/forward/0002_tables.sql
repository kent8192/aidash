SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE TABLE public.marketplace_audiences (
    key text NOT NULL,
    document jsonb NOT NULL
);

--

CREATE TABLE public.marketplace_consents (
    key text NOT NULL,
    document jsonb NOT NULL
);

--

CREATE TABLE public.marketplace_installations (
    key text NOT NULL,
    document jsonb NOT NULL,
    tenant text NOT NULL,
    package_key text NOT NULL
);

ALTER TABLE ONLY public.marketplace_installations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.marketplace_provenance (
    key text NOT NULL,
    document jsonb NOT NULL
);

--

CREATE TABLE public.marketplace_requests (
    key text NOT NULL,
    tenant text NOT NULL,
    document jsonb NOT NULL
);

ALTER TABLE ONLY public.marketplace_requests FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.marketplace_revisions (
    tenant text NOT NULL,
    key text NOT NULL,
    document jsonb NOT NULL,
    installation text NOT NULL,
    revision bigint NOT NULL,
    entry_id text NOT NULL,
    entry_version text NOT NULL
);

ALTER TABLE ONLY public.marketplace_revisions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.marketplace_versions (
    key text NOT NULL,
    document jsonb NOT NULL,
    repository text NOT NULL,
    owner text NOT NULL,
    package_id text NOT NULL,
    version text NOT NULL,
    kind text NOT NULL,
    source_id text NOT NULL,
    source_version text NOT NULL,
    source_content text NOT NULL
);

--

ALTER TABLE ONLY public.marketplace_audiences
    ADD CONSTRAINT marketplace_audiences_pkey PRIMARY KEY (key);

--

ALTER TABLE ONLY public.marketplace_consents
    ADD CONSTRAINT marketplace_consents_pkey PRIMARY KEY (key);

--

ALTER TABLE ONLY public.marketplace_installations
    ADD CONSTRAINT marketplace_installations_pkey PRIMARY KEY (key);

--

ALTER TABLE ONLY public.marketplace_installations
    ADD CONSTRAINT marketplace_installations_tenant_key_key UNIQUE (tenant, key);

--

ALTER TABLE ONLY public.marketplace_installations
    ADD CONSTRAINT marketplace_installations_tenant_package_key_key UNIQUE (tenant, package_key);

--

ALTER TABLE ONLY public.marketplace_provenance
    ADD CONSTRAINT marketplace_provenance_pkey PRIMARY KEY (key);

--

ALTER TABLE ONLY public.marketplace_requests
    ADD CONSTRAINT marketplace_requests_pkey PRIMARY KEY (tenant, key);

--

ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_entry_id_entry_version_key UNIQUE (tenant, entry_id, entry_version);

--

ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_installation_revision_key UNIQUE (installation, revision);

--

ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_pkey PRIMARY KEY (key);

--

ALTER TABLE ONLY public.marketplace_versions
    ADD CONSTRAINT marketplace_versions_pkey PRIMARY KEY (key);

--

ALTER TABLE ONLY public.marketplace_versions
    ADD CONSTRAINT marketplace_versions_repository_owner_package_id_version_key UNIQUE (repository, owner, package_id, version);

--

ALTER TABLE ONLY public.marketplace_revisions
    ADD CONSTRAINT marketplace_revisions_installation_fkey FOREIGN KEY (tenant, installation) REFERENCES public.marketplace_installations(tenant, key);

--

--

--

--

--

--

--
