DROP POLICY tenant_isolation ON public.node_deletions;
ALTER TABLE public.node_deletions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.node_deletions DISABLE ROW LEVEL SECURITY;
