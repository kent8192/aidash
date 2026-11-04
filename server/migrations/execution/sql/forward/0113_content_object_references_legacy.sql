SET LOCAL aidash.scope = 'deployment';
-- The undeployed cutover discards existing Working Area contents.
INSERT INTO public.core_manifest_revisions (tenant, area_id, revision, manifest)
SELECT tenant, id, revision, '[]'::jsonb FROM public.core_areas;
DROP TRIGGER atomic_write_guard ON public.core_quotas;
DROP TRIGGER atomic_write_guard ON public.core_objects;
DROP POLICY tenant_isolation ON public.core_retention_files;
DROP POLICY tenant_isolation ON public.core_quotas;
DROP POLICY tenant_isolation ON public.core_objects;
