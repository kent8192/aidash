DROP POLICY tenant_isolation ON public.connection_scope_ceilings;
ALTER TABLE public.connection_scope_ceilings NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.connection_scope_ceilings DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.oauth_clients;
ALTER TABLE public.oauth_clients NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.oauth_clients DISABLE ROW LEVEL SECURITY;
