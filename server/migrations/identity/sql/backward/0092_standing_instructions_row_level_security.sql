DROP POLICY tenant_isolation ON public.node_instructions;
ALTER TABLE public.node_instructions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.node_instructions DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.tenant_instructions;
ALTER TABLE public.tenant_instructions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.tenant_instructions DISABLE ROW LEVEL SECURITY;
