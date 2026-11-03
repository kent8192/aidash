DROP POLICY tenant_isolation ON public.semantic_image_publications;
ALTER TABLE public.semantic_image_publications NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_image_publications DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.semantic_image_ledgers;
ALTER TABLE public.semantic_image_ledgers NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.semantic_image_ledgers DISABLE ROW LEVEL SECURITY;
