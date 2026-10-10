CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.inference_attempts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.inference_progress FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
