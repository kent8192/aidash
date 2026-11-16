DROP POLICY tenant_isolation ON public.agent_plugin_data;
ALTER TABLE public.agent_plugin_data NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.agent_plugin_data DISABLE ROW LEVEL SECURITY;
