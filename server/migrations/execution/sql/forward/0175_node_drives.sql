-- Reinhardt cannot define a trigger or a set-based seed from another table.
SET LOCAL aidash.scope = 'deployment';
INSERT INTO public.drives (id, tenant, node, closed, head)
SELECT gen_random_uuid(), tenant, name, false, 0 FROM public.nodes
WHERE NOT EXISTS (
    SELECT 1 FROM public.node_deletions d
    WHERE d.tenant = nodes.tenant AND d.node = nodes.name AND d.purged_at IS NOT NULL
);
CREATE FUNCTION public.aidash_create_node_drive() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
BEGIN
    INSERT INTO public.drives (id, tenant, node, closed, head)
    VALUES (gen_random_uuid(), NEW.tenant, NEW.name, false, 0);
    RETURN NEW;
END
$$;
CREATE TRIGGER aidash_create_node_drive
AFTER INSERT ON public.nodes FOR EACH ROW EXECUTE FUNCTION public.aidash_create_node_drive();
ALTER TABLE public.drives ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drives FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drives AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_versions AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_entries FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_entries AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_trash ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_trash FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_trash AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_uploads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_uploads AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_upload_parts ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_upload_parts FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_upload_parts AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_jobs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
ALTER TABLE public.drive_job_blobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.drive_job_blobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.drive_job_blobs AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
