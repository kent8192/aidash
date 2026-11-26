DROP POLICY tenant_isolation ON public.drive_index_requests;
ALTER TABLE public.drive_index_requests NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.drive_index_requests DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.drive_passages;
ALTER TABLE public.drive_passages NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.drive_passages DISABLE ROW LEVEL SECURITY;
