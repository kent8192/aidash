SET LOCAL search_path = public, pg_catalog;

CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_audiences FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_consents FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_gate FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_installations FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_provenance FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_requests FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_revisions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON public.marketplace_versions FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER marketplace_immutable BEFORE DELETE OR UPDATE ON public.marketplace_revisions FOR EACH ROW EXECUTE FUNCTION public.marketplace_immutable();
CREATE TRIGGER marketplace_immutable BEFORE DELETE OR UPDATE ON public.marketplace_versions FOR EACH ROW EXECUTE FUNCTION public.marketplace_immutable();
