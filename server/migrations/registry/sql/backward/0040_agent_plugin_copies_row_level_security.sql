DROP POLICY tenant_isolation ON public.agent_plugin_copies;
ALTER TABLE public.agent_plugin_copies NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.agent_plugin_copies DISABLE ROW LEVEL SECURITY;
