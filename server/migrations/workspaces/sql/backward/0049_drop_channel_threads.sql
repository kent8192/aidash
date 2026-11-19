-- Restore the empty reply-thread table (#1008): its rows are not restored. The
-- table carries a forced row-level policy and a write guard that typed
-- CreateTable cannot express.
CREATE TABLE public.channel_threads (
    id uuid NOT NULL,
    tenant text NOT NULL,
    workspace_id uuid NOT NULL,
    root_message_id uuid NOT NULL,
    created_by text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);
ALTER TABLE ONLY public.channel_threads FORCE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.channel_threads ADD CONSTRAINT channel_thread_scope_key UNIQUE (workspace_id, id);
ALTER TABLE ONLY public.channel_threads ADD CONSTRAINT channel_threads_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.channel_threads ADD CONSTRAINT channel_threads_root_message_id_key UNIQUE (root_message_id);
ALTER TABLE ONLY public.channel_threads ADD CONSTRAINT channel_threads_tenant_workspace_id_id_key UNIQUE (tenant, workspace_id, id);
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.channel_threads FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
ALTER TABLE ONLY public.channel_threads ADD CONSTRAINT channel_threads_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE ONLY public.channel_threads ADD CONSTRAINT channel_threads_workspace_id_root_message_id_fkey FOREIGN KEY (tenant, workspace_id, root_message_id) REFERENCES public.messages(tenant, workspace_id, id) ON DELETE CASCADE;
ALTER TABLE public.channel_threads ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.channel_threads USING (((SELECT public.aidash_deployment_scope()) OR (tenant = (SELECT public.aidash_scope_tenant())))) WITH CHECK (((SELECT public.aidash_deployment_scope()) OR (tenant = (SELECT public.aidash_scope_tenant()))));
