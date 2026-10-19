DROP POLICY tenant_isolation ON public.remote_run_message_fences;
ALTER TABLE public.remote_run_message_fences NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.remote_run_message_fences DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.delegations;
ALTER TABLE public.delegations NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.delegations DISABLE ROW LEVEL SECURITY;
DROP POLICY tenant_isolation ON public.authorization_remote_grant_reads;
ALTER TABLE public.authorization_remote_grant_reads NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.authorization_remote_grant_reads DISABLE ROW LEVEL SECURITY;
