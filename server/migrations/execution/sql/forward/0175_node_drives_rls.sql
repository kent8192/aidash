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
