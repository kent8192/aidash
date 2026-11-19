-- Restore the empty Task to reply-thread map (#1008): its rows are not
-- restored. The table carries a forced row-level policy and a write guard
-- that typed CreateTable cannot express.
CREATE TABLE public.core_task_sessions (
    task_id uuid NOT NULL,
    tenant text NOT NULL,
    thread_id uuid NOT NULL
);
ALTER TABLE ONLY public.core_task_sessions FORCE ROW LEVEL SECURITY;
ALTER TABLE ONLY public.core_task_sessions ADD CONSTRAINT core_task_sessions_pkey PRIMARY KEY (task_id);
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.core_task_sessions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
ALTER TABLE ONLY public.core_task_sessions ADD CONSTRAINT core_task_sessions_tenant_root_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
ALTER TABLE public.core_task_sessions ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_task_sessions USING (((SELECT public.aidash_deployment_scope()) OR (tenant = (SELECT public.aidash_scope_tenant())))) WITH CHECK (((SELECT public.aidash_deployment_scope()) OR (tenant = (SELECT public.aidash_scope_tenant()))));
