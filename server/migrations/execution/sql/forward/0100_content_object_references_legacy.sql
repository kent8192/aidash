DROP TRIGGER atomic_write_guard ON public.core_quotas;
DROP TRIGGER atomic_write_guard ON public.core_objects;
DROP POLICY tenant_isolation ON public.core_retention_files;
DROP POLICY tenant_isolation ON public.core_quotas;
DROP POLICY tenant_isolation ON public.core_objects;
