-- Restored Working Areas each received a fresh session id (#1008).
ALTER TABLE public.core_areas ALTER COLUMN thread_id DROP DEFAULT;
