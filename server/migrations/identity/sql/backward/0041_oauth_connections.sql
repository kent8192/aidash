DROP POLICY tenant_isolation ON public.oauth_dynamic_clients;
ALTER TABLE public.oauth_dynamic_clients NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_dynamic_clients DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.oauth_authorizations;
ALTER TABLE public.oauth_authorizations NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_authorizations DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.oauth_connections;
ALTER TABLE public.oauth_connections NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_connections DISABLE ROW LEVEL SECURITY;
