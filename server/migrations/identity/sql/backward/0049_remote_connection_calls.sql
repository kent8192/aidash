DROP POLICY tenant_isolation ON public.remote_connection_calls;
ALTER TABLE public.remote_connection_calls NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.remote_connection_calls DISABLE ROW LEVEL SECURITY;
