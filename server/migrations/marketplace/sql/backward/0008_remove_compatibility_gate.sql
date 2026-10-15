CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_gate FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
