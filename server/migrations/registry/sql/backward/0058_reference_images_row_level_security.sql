DROP POLICY tenant_isolation ON public.agent_knowledge_images;
ALTER TABLE public.agent_knowledge_images NO FORCE ROW LEVEL SECURITY;
ALTER TABLE public.agent_knowledge_images DISABLE ROW LEVEL SECURITY;
