DROP POLICY tenant_isolation ON public.model_calls;
ALTER TABLE public.model_calls NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.model_calls DISABLE ROW LEVEL SECURITY;
