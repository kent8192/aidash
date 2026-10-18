-- The tenancy scope that server/src/database/scope.rs writes after BEGIN
-- (server/migrations/README.md) decides which Tenant-owned rows a transaction
-- reaches. SQL does not promise that OR short-circuits, so each function is
-- safe in every scope a policy can see: the Tenant function returns NULL in a
-- Deployment Scope instead of raising. Both raise SQLSTATE TS001 when the
-- transaction has no usable scope. The message names the scope state, never a
-- Tenant.
CREATE FUNCTION public.aidash_deployment_scope() RETURNS boolean
    LANGUAGE plpgsql STABLE PARALLEL SAFE
    SET search_path = pg_catalog
AS $aidash_scope$
DECLARE
    scope text := coalesce(current_setting('aidash.scope', true), '');
BEGIN
    IF scope = 'deployment' THEN
        RETURN true;
    ELSIF scope = 'tenant' THEN
        RETURN false;
    ELSIF scope = 'none' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is none';
    ELSIF scope = '' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unset';
    ELSE
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unknown';
    END IF;
END
$aidash_scope$;

CREATE FUNCTION public.aidash_scope_tenant() RETURNS text
    LANGUAGE plpgsql STABLE PARALLEL SAFE
    SET search_path = pg_catalog
AS $aidash_scope$
DECLARE
    scope text := coalesce(current_setting('aidash.scope', true), '');
    tenant text := coalesce(current_setting('aidash.tenant', true), '');
BEGIN
    IF scope = 'tenant' THEN
        IF tenant = '' THEN
            RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
                USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is tenant without a Tenant name';
        END IF;
        RETURN tenant;
    ELSIF scope = 'deployment' THEN
        RETURN NULL;
    ELSIF scope = 'none' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is none';
    ELSIF scope = '' THEN
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unset';
    ELSE
        RAISE EXCEPTION 'Tenant-owned data needs a Tenant or Deployment Scope'
            USING ERRCODE = 'TS001', DETAIL = 'aidash.scope is unknown';
    END IF;
END
$aidash_scope$;
