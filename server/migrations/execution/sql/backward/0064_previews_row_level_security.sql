DROP POLICY tenant_isolation ON public.core_previews;
ALTER TABLE public.core_previews NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_previews DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_preview_tickets;
ALTER TABLE public.core_preview_tickets NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_preview_tickets DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_preview_sessions;
ALTER TABLE public.core_preview_sessions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_preview_sessions DISABLE ROW LEVEL SECURITY;
