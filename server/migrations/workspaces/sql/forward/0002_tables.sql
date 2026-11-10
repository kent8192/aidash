SET LOCAL check_function_bodies = false;
SET LOCAL search_path = public, pg_catalog;
--

CREATE FUNCTION public.guard_task_dependencies() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        IF NOT public.aidash_tenant_purge() THEN
            RAISE EXCEPTION 'task_dependencies is maintained by tasks'
                USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependencies_managed';
        END IF;
    END IF;
    RETURN NULL;
END $$;

--

CREATE FUNCTION public.guard_task_dependency_cycle() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS (
        WITH RECURSIVE edges(source_id, target_id) AS (
            SELECT parent_id, id FROM tasks WHERE parent_id IS NOT NULL
            UNION ALL
            SELECT task_id, dependency_id FROM task_dependencies
        ), reachable(current_id) AS (
            SELECT target_id FROM edges WHERE source_id = NEW.id
            UNION
            SELECT edge.target_id
            FROM reachable r
            JOIN edges edge ON edge.source_id = r.current_id
        )
        SELECT 1 FROM reachable WHERE current_id = NEW.id
    ) THEN
        RAISE EXCEPTION 'task dependency graph contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependency_cycle';
    END IF;
    RETURN NEW;
END $$;

--

CREATE FUNCTION public.guard_task_parent_cycle() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- The statement trigger takes the shared lock before deferred-check
    -- snapshots are established; keep this acquisition as a defensive guard.
    -- Scope serialization to this schema's task hierarchy. Isolated tenant/test
    -- schemas share a database but must not block one another's graph writes.
    PERFORM pg_advisory_xact_lock(70721021, hashtext(TG_TABLE_SCHEMA));
    IF EXISTS (
        WITH RECURSIVE walk(current_id, parent_id, path, cycle) AS (
            SELECT id, parent_id, ARRAY[id], false
            FROM tasks
            WHERE id = NEW.id
            UNION ALL
            SELECT t.id, t.parent_id, w.path || t.id, t.id = ANY(w.path)
            FROM walk w
            JOIN tasks t ON t.id = w.parent_id
            WHERE NOT w.cycle
        )
        SELECT 1 FROM walk WHERE cycle
    ) THEN
        RAISE EXCEPTION 'task parent hierarchy contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_parent_cycle';
    END IF;
    RETURN NEW;
END $$;

--

CREATE FUNCTION public.lock_task_hierarchy_before_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(70721021, hashtext(TG_TABLE_SCHEMA));
    RETURN NULL;
END $$;

--

CREATE FUNCTION public.sync_task_dependencies() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        DELETE FROM task_dependencies WHERE task_id = OLD.id;
    END IF;
    INSERT INTO task_dependencies(task_id, tenant, workspace_id, dependency_id)
        SELECT DISTINCT NEW.id, NEW.tenant, NEW.workspace_id, unnest(NEW.dependencies);
    RETURN NEW;
END $$;

--

CREATE TABLE public.artifacts (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    task_id uuid NOT NULL,
    kind text NOT NULL,
    name text NOT NULL,
    content jsonb NOT NULL,
    created_by text NOT NULL,
    idempotency_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    run_id uuid,
    CONSTRAINT artifacts_kind_check CHECK ((kind = ANY (ARRAY['text'::text, 'json'::text, 'file_reference'::text, 'code'::text, 'structured_result'::text])))
);

ALTER TABLE ONLY public.artifacts FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.channel_attachments (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    uploaded_by text NOT NULL,
    idempotency_key uuid NOT NULL,
    filename text NOT NULL,
    media_type text NOT NULL,
    sha256 text NOT NULL,
    size_bytes bigint NOT NULL,
    content bytea,
    message_id uuid,
    "position" integer DEFAULT 0 NOT NULL,
    object_id uuid,
    width integer,
    height integer,
    duration_ms bigint,
    wav_object_id uuid,
    wav_sha256 text,
    wav_size_bytes bigint,
    CONSTRAINT channel_attachments_has_content CHECK (((object_id IS NOT NULL) OR (content IS NOT NULL))),
    CONSTRAINT channel_attachments_wav_complete CHECK ((((wav_object_id IS NULL) AND (wav_sha256 IS NULL) AND (wav_size_bytes IS NULL)) OR ((wav_object_id IS NOT NULL) AND (wav_sha256 IS NOT NULL) AND (wav_size_bytes IS NOT NULL))))
);

ALTER TABLE ONLY public.channel_attachments FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.channel_message_context (
    message_id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    thread_id uuid,
    attachment_digest text DEFAULT 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'::text NOT NULL
);

ALTER TABLE ONLY public.channel_message_context FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.channel_threads (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    root_message_id uuid NOT NULL,
    created_by text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

ALTER TABLE ONLY public.channel_threads FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.conversations (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    target text NOT NULL,
    target_kind text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    created_by text DEFAULT 'human'::text NOT NULL,
    scratch_space_id uuid NOT NULL,
    CONSTRAINT conversations_target_kind_check CHECK ((target_kind = ANY (ARRAY['agent'::text, 'cluster'::text])))
);

ALTER TABLE ONLY public.conversations FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.message_file_versions (
    id uuid NOT NULL,
    message_id uuid NOT NULL,
    file_id uuid NOT NULL,
    tenant text NOT NULL,
    area_id uuid NOT NULL,
    revision bigint NOT NULL,
    path text NOT NULL,
    digest text NOT NULL,
    size bigint NOT NULL,
    media_type text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    remote_run_id uuid,
    remote_expires_at timestamp with time zone,
    CONSTRAINT message_file_version_size CHECK ((size >= 0))
);

ALTER TABLE ONLY public.message_file_versions FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.messages (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    sender text NOT NULL,
    content text NOT NULL,
    idempotency_key text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    run_id uuid
);

ALTER TABLE ONLY public.messages FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.scratch_space_members (
    id uuid NOT NULL,
    tenant text NOT NULL,
    space_id uuid NOT NULL,
    subject text NOT NULL
);

ALTER TABLE ONLY public.scratch_space_members FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.scratch_space_settings (
    space_id uuid NOT NULL,
    tenant text NOT NULL,
    settings jsonb NOT NULL
);

ALTER TABLE ONLY public.scratch_space_settings FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.scratch_spaces (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    name text NOT NULL,
    creator text NOT NULL,
    state text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    wipe_generation bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone NOT NULL,
    last_used_at timestamp with time zone NOT NULL
);

ALTER TABLE ONLY public.scratch_spaces FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.task_dependencies (
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    dependency_id uuid NOT NULL
);

ALTER TABLE ONLY public.task_dependencies FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.tasks (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    title text NOT NULL,
    description text NOT NULL,
    status text DEFAULT 'OPEN'::text NOT NULL,
    requirements jsonb DEFAULT '{}'::jsonb NOT NULL,
    owner text,
    created_by text NOT NULL,
    dependencies uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    parent_id uuid,
    revision bigint DEFAULT 0 NOT NULL,
    creation_key text,
    completion_key text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT tasks_active_owner CHECK (COALESCE(((status <> ALL (ARRAY['CLAIMED'::text, 'RUNNING'::text])) OR (owner IS NOT NULL)), false)),
    CONSTRAINT tasks_check CHECK ((((status = 'OPEN'::text) AND (owner IS NULL)) OR (status <> 'OPEN'::text))),
    CONSTRAINT tasks_content CHECK (COALESCE(((length(btrim(title, E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (length(btrim(description, E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (jsonb_typeof(requirements) = 'object'::text) AND (revision >= 0) AND (revision < '9223372036854775807'::bigint) AND
CASE
    WHEN (jsonb_typeof(requirements) = 'object'::text) THEN ((requirements - ARRAY['kind'::text, 'query'::text, 'capability'::text, 'language'::text, 'skill'::text, 'tag'::text, 'model'::text]) = '{}'::jsonb)
    ELSE false
END AND (NOT jsonb_path_exists(requirements, 'strict $.*?(@.type() != "string" && @.type() != "null")'::jsonpath, '{}'::jsonb, true))), false)),
    CONSTRAINT tasks_no_self_reference CHECK (COALESCE((((parent_id IS NULL) OR (parent_id <> id)) AND (NOT (id = ANY (dependencies))) AND (array_position(dependencies, NULL::uuid) IS NULL)), false)),
    CONSTRAINT tasks_status_check CHECK ((status = ANY (ARRAY['OPEN'::text, 'CLAIMED'::text, 'RUNNING'::text, 'COMPLETED'::text, 'FAILED'::text, 'BLOCKED'::text, 'CANCELLED'::text, 'ABANDONED'::text])))
);

ALTER TABLE ONLY public.tasks FORCE ROW LEVEL SECURITY;

--

CREATE TABLE public.workspaces (
    id uuid NOT NULL,
    tenant text NOT NULL,
    node text NOT NULL,
    title text NOT NULL,
    goal text NOT NULL,
    state jsonb DEFAULT '{}'::jsonb NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    scratch_space_id uuid NOT NULL,
    created_by text NOT NULL,
    CONSTRAINT workspaces_content CHECK (COALESCE(((length(btrim(title, E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (length(btrim(goal, E'\u0009\u000a\u000b\u000c\u000d\u0020\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (jsonb_typeof(state) = 'object'::text) AND (revision >= 0)), false))
);

ALTER TABLE ONLY public.workspaces FORCE ROW LEVEL SECURITY;

--

ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_idempotency_key_key UNIQUE (idempotency_key);

--

ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachment_idempotency UNIQUE (workspace_id, uploaded_by, idempotency_key);

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachment_scope_key UNIQUE (workspace_id, id);

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.channel_message_context
    ADD CONSTRAINT channel_message_context_pkey PRIMARY KEY (message_id);

--

ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_thread_scope_key UNIQUE (workspace_id, id);

--

ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_root_message_id_key UNIQUE (root_message_id);

--

ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_tenant_workspace_id_id_key UNIQUE (tenant, workspace_id, id);

--

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.message_file_versions
    ADD CONSTRAINT message_file_versions_message_file_key UNIQUE (message_id, file_id);

--

ALTER TABLE ONLY public.message_file_versions
    ADD CONSTRAINT message_file_versions_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_idempotency_key_key UNIQUE (idempotency_key);

--

ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_tenant_workspace_id_id_key UNIQUE (tenant, workspace_id, id);

--

ALTER TABLE ONLY public.scratch_space_members
    ADD CONSTRAINT scratch_space_members_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.scratch_space_members
    ADD CONSTRAINT scratch_space_members_subject_key UNIQUE (space_id, subject);

--

ALTER TABLE ONLY public.scratch_space_settings
    ADD CONSTRAINT scratch_space_settings_pkey PRIMARY KEY (space_id);

--

ALTER TABLE ONLY public.scratch_spaces
    ADD CONSTRAINT scratch_spaces_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.scratch_spaces
    ADD CONSTRAINT scratch_spaces_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT task_dependencies_pkey PRIMARY KEY (task_id, dependency_id);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_completion_key_key UNIQUE (completion_key);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_creation_key_key UNIQUE (creation_key);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_tenant_id_key UNIQUE (tenant, id);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_tenant_id_workspace_id_key UNIQUE (tenant, id, workspace_id);

--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_pkey PRIMARY KEY (id);

--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_tenant_id_key UNIQUE (tenant, id);

--

CREATE UNIQUE INDEX channel_message_scope_key ON public.messages USING btree (workspace_id, id);

--

CREATE UNIQUE INDEX tasks_id_workspace_unique ON public.tasks USING btree (id, workspace_id);

--

ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_task_id_fkey FOREIGN KEY (tenant, task_id) REFERENCES public.tasks(tenant, id);

--

ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_task_workspace FOREIGN KEY (tenant, task_id, workspace_id) REFERENCES public.tasks(tenant, id, workspace_id);

--

ALTER TABLE ONLY public.artifacts
    ADD CONSTRAINT artifacts_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.channel_attachments
    ADD CONSTRAINT channel_attachments_workspace_id_message_id_fkey FOREIGN KEY (tenant, workspace_id, message_id) REFERENCES public.messages(tenant, workspace_id, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.channel_message_context
    ADD CONSTRAINT channel_message_context_workspace_id_message_id_fkey FOREIGN KEY (tenant, workspace_id, message_id) REFERENCES public.messages(tenant, workspace_id, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.channel_message_context
    ADD CONSTRAINT channel_message_context_workspace_id_thread_id_fkey FOREIGN KEY (tenant, workspace_id, thread_id) REFERENCES public.channel_threads(tenant, workspace_id, id);

--

ALTER TABLE ONLY public.channel_threads
    ADD CONSTRAINT channel_threads_workspace_id_root_message_id_fkey FOREIGN KEY (tenant, workspace_id, root_message_id) REFERENCES public.messages(tenant, workspace_id, id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_scratch_space_fkey FOREIGN KEY (tenant, scratch_space_id) REFERENCES public.scratch_spaces(tenant, id) ON UPDATE RESTRICT;

--

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.message_file_versions
    ADD CONSTRAINT message_file_versions_message_fkey FOREIGN KEY (tenant, message_id) REFERENCES public.messages(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.messages
    ADD CONSTRAINT messages_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.scratch_space_members
    ADD CONSTRAINT scratch_space_members_space_fkey FOREIGN KEY (tenant, space_id) REFERENCES public.scratch_spaces(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.scratch_space_settings
    ADD CONSTRAINT scratch_space_settings_space_fkey FOREIGN KEY (tenant, space_id) REFERENCES public.scratch_spaces(tenant, id) ON UPDATE RESTRICT ON DELETE CASCADE;

--

ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT tasks_dependencies_source_workspace FOREIGN KEY (tenant, task_id, workspace_id) REFERENCES public.tasks(tenant, id, workspace_id) ON DELETE CASCADE;

--

ALTER TABLE ONLY public.task_dependencies
    ADD CONSTRAINT tasks_dependencies_target_workspace FOREIGN KEY (tenant, dependency_id, workspace_id) REFERENCES public.tasks(tenant, id, workspace_id);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_parent_id_fkey FOREIGN KEY (tenant, parent_id) REFERENCES public.tasks(tenant, id);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_parent_workspace FOREIGN KEY (tenant, parent_id, workspace_id) REFERENCES public.tasks(tenant, id, workspace_id);

--

ALTER TABLE ONLY public.tasks
    ADD CONSTRAINT tasks_workspace_id_fkey FOREIGN KEY (tenant, workspace_id) REFERENCES public.workspaces(tenant, id);

--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_scratch_space_fkey FOREIGN KEY (tenant, scratch_space_id) REFERENCES public.scratch_spaces(tenant, id) ON UPDATE RESTRICT;

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
