CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.semantic_agent_memory FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.memory FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
