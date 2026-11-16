DROP POLICY tenant_isolation ON public.memory_journal_proofs;
ALTER TABLE public.memory_journal_proofs NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.memory_journal_proofs DISABLE ROW LEVEL SECURITY;
