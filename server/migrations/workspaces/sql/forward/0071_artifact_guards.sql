DROP TRIGGER atomic_write_guard ON public.artifacts;
DROP POLICY tenant_isolation ON public.artifacts;
ALTER TABLE public.artifacts NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.artifacts DISABLE ROW LEVEL SECURITY;
