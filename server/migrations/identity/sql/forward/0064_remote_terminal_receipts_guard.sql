CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.authorization_remote_terminal_receipts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
