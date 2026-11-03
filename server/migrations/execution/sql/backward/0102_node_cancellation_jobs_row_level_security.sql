DROP POLICY tenant_isolation ON public.node_cancellation_jobs;
ALTER TABLE public.node_cancellation_jobs NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.node_cancellation_jobs DISABLE ROW LEVEL SECURITY;
