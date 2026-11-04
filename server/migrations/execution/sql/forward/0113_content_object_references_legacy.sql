SET LOCAL aidash.scope = 'deployment';
-- Old UUID-backed object bytes are discarded at this undeployed cutover.
-- Delete rows that name those objects before rebuilding their foreign keys.
DELETE FROM public.channel_attachments
 WHERE object_id IS NOT NULL OR wav_object_id IS NOT NULL;
DELETE FROM public.core_records
 WHERE kind IN ('reference', 'transfer_in', 'transfer_out', 'collaboration', 'cleanup');
-- The undeployed cutover discards existing Working Area contents.
INSERT INTO public.core_manifest_revisions (tenant, area_id, revision, manifest)
SELECT tenant, id, revision, '[]'::jsonb FROM public.core_areas;
DROP TRIGGER atomic_write_guard ON public.core_quotas;
DROP TRIGGER atomic_write_guard ON public.core_objects;
DROP POLICY tenant_isolation ON public.core_retention_files;
DROP POLICY tenant_isolation ON public.core_quotas;
DROP POLICY tenant_isolation ON public.core_objects;
