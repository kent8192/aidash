DROP POLICY tenant_isolation ON public.marketplace_revisions;
ALTER TABLE public.marketplace_revisions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.marketplace_revisions DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.marketplace_requests;
ALTER TABLE public.marketplace_requests NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.marketplace_requests DISABLE ROW LEVEL SECURITY;
