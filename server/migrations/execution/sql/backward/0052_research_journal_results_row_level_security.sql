DROP POLICY tenant_isolation ON public.research_journal_results;
ALTER TABLE public.research_journal_results NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.research_journal_results DISABLE ROW LEVEL SECURITY;
