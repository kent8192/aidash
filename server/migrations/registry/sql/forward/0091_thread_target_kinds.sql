ALTER TABLE public.conversations DROP CONSTRAINT conversations_target_kind_check;
ALTER TABLE public.conversations ADD CONSTRAINT conversations_target_kind_check
 CHECK (target_kind IN ('agent', 'node')) NOT VALID;
