-- A Live preview's request has no core operation; the session id names it.
-- The pinned typed AlterColumn renders no NOT NULL change for PostgreSQL
-- (kent8192/reinhardt-web#6803), so this is a documented RunSQL exception.
ALTER TABLE public.egress_requests ALTER COLUMN operation_id DROP NOT NULL;
