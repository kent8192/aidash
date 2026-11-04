-- Reverse cutover drops content-addressed objects; remove rows that name them.
SET LOCAL aidash.scope = 'deployment';
DELETE FROM public.channel_attachments WHERE object_id IS NOT NULL OR wav_object_id IS NOT NULL;
DELETE FROM public.core_records WHERE kind IN ('reference', 'transfer_in', 'transfer_out', 'collaboration', 'cleanup');
DROP TRIGGER atomic_write_guard ON public.core_quotas;
DROP TRIGGER atomic_write_guard ON public.core_objects;
