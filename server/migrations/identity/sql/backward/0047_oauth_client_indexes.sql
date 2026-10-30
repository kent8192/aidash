DROP INDEX public.oauth_clients_owner_issuer;
DROP INDEX public.tenant_secrets_live_name;
CREATE UNIQUE INDEX tenant_secrets_live_name ON public.tenant_secrets (tenant, name) WHERE state <> 'deleted';
