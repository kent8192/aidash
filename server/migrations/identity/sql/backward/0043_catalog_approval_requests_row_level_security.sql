DROP POLICY tenant_isolation ON public.catalog_approval_requests;
ALTER TABLE public.catalog_approval_requests NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.catalog_approval_requests DISABLE ROW LEVEL SECURITY;
