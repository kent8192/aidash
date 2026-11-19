CREATE TABLE public.schedule_limits (
    tenant text NOT NULL PRIMARY KEY,
    max_schedules integer NOT NULL,
    max_per_person integer NOT NULL,
    min_interval_seconds bigint NOT NULL,
    revision bigint NOT NULL DEFAULT 0,
    updated_at timestamp with time zone NOT NULL,
    updated_by text NOT NULL,
    CONSTRAINT schedule_limits_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name) ON DELETE CASCADE ON UPDATE RESTRICT,
    CONSTRAINT schedule_limits_values_check CHECK (max_schedules >= 1 AND max_per_person >= 1 AND min_interval_seconds >= 60)
);
ALTER TABLE public.schedule_limits ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.schedule_limits FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.schedule_limits AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
