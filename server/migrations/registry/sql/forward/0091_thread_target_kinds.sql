SET LOCAL aidash.scope = 'deployment';

-- Apply the existing Thread deletion tombstone to retired targets. PostgreSQL
-- function bodies and the ALTER TYPE ... USING rewrite are untyped DDL.
CREATE FUNCTION public.aidash_retire_cluster_thread_0091(
 thread uuid, owner_tenant text, previous timestamptz
) RETURNS timestamptz
LANGUAGE sql STABLE AS $$
 SELECT CASE WHEN previous IS NULL AND EXISTS (
  SELECT FROM public.conversations c
   WHERE c.thread_id = thread AND c.tenant = owner_tenant AND c.target_kind = 'cluster'
 ) THEN CURRENT_TIMESTAMP ELSE previous END
$$;
ALTER TABLE public.threads ALTER COLUMN deleted_at TYPE timestamptz
 USING public.aidash_retire_cluster_thread_0091(id, tenant, deleted_at);
DROP FUNCTION public.aidash_retire_cluster_thread_0091(uuid, text, timestamptz);

ALTER TABLE public.conversations DROP CONSTRAINT conversations_target_kind_check;
ALTER TABLE public.conversations ADD CONSTRAINT conversations_target_kind_check
 CHECK (target_kind IN ('agent', 'node')) NOT VALID;
