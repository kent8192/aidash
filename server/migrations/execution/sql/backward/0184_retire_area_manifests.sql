CREATE TABLE public.core_manifest_revisions (
    tenant text NOT NULL,
    area_id uuid NOT NULL,
    revision bigint NOT NULL,
    manifest jsonb NOT NULL,
    CONSTRAINT core_manifest_revisions_pkey PRIMARY KEY (tenant, area_id, revision),
    CONSTRAINT core_manifest_revisions_tenant_root_fkey FOREIGN KEY (tenant)
        REFERENCES public.tenants (name),
    CONSTRAINT core_manifest_revisions_area_fkey FOREIGN KEY (tenant, area_id)
        REFERENCES public.core_areas (tenant, id) ON DELETE CASCADE ON UPDATE RESTRICT
);
ALTER TABLE public.core_manifest_revisions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.core_manifest_revisions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.core_manifest_revisions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
