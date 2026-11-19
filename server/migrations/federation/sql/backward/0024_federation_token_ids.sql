DROP POLICY tenant_isolation ON public.federation_token_ids;
ALTER TABLE public.federation_token_ids NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.federation_token_ids DISABLE ROW LEVEL SECURITY;
