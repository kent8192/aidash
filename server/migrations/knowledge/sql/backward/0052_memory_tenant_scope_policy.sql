DROP POLICY tenant_isolation ON public.memory_tenant_scope;
ALTER TABLE public.memory_tenant_scope NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.memory_tenant_scope DISABLE ROW LEVEL SECURITY;
