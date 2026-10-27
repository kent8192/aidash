-- Reversing to a required operation id needs every session request removed
-- first; the DDL fails rather than inventing an operation that never existed.
ALTER TABLE public.egress_requests ALTER COLUMN operation_id SET NOT NULL;
