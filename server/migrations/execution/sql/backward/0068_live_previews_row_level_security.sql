DROP POLICY tenant_isolation ON public.egress_sessions;
ALTER TABLE public.egress_sessions NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.egress_sessions DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_preview_limits;
ALTER TABLE public.core_preview_limits NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_preview_limits DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.core_live_previews;
ALTER TABLE public.core_live_previews NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.core_live_previews DISABLE ROW LEVEL SECURITY;
