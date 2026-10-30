-- Refuse reversal while any object-backed attachment has no inline bytes.
ALTER TABLE public.channel_attachments ALTER COLUMN content SET NOT NULL;
