DROP POLICY tenant_isolation ON public.scratch_space_members;
ALTER TABLE public.scratch_space_members NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.scratch_space_members DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.scratch_space_settings;
ALTER TABLE public.scratch_space_settings NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.scratch_space_settings DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.scratch_spaces;
ALTER TABLE public.scratch_spaces NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.scratch_spaces DISABLE ROW LEVEL SECURITY;
