-- Reinhardt's typed AlterColumn does not change PostgreSQL nullability (#6803).
ALTER TABLE public.channel_attachments ALTER COLUMN content DROP NOT NULL;
