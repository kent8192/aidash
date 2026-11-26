DROP POLICY tenant_isolation ON public.tenant_embedding_settings;
ALTER TABLE public.tenant_embedding_settings NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_embedding_settings DISABLE ROW LEVEL SECURITY;
