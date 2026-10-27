DROP INDEX public.tenant_secrets_live_name;
CREATE UNIQUE INDEX tenant_secrets_live_name ON public.tenant_secrets (tenant, coalesce(owner,''), name) WHERE state <> 'deleted';
CREATE UNIQUE INDEX oauth_clients_owner_issuer ON public.oauth_clients (tenant, coalesce(owner,''), issuer);
